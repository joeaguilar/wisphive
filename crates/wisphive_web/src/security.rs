//! Security layer for the web UI.
//!
//! Gates `/ws` and `/api/*` routes behind three checks:
//!
//! 1. **Host header allowlist** — defeats DNS rebinding. The daemon is bound
//!    to localhost (or a LAN IP), but a rebound domain would send a
//!    non-local `Host` header, which we reject.
//! 2. **Origin header allowlist** — when present, must match our configured
//!    origins. Same-origin navigations to `/` don't include `Origin`, so we
//!    don't require it — but any cross-origin fetch (CSRF, WebSocket) will.
//! 3. **Device bearer token** — per-device random bearer tokens issued by
//!    `/api/auth/login` after a valid password. Only the SHA-256 hash is
//!    persisted in `web_devices`; the raw token is shown to the client
//!    exactly once. Every `/api/*` (except `/api/auth/login`) and `/ws`
//!    request MUST carry a matching, non-revoked token via
//!    `Authorization: Bearer <raw>` or `?token=<raw>`.
//!
//! Together these defeat the CVSS ~9.0 vector where a malicious web page
//! opens `ws://127.0.0.1:3100/ws` and drives the daemon with full Tui
//! privileges.
//!
//! The old per-process `web.token` file and `/api/web-token` bootstrap are
//! retired: they shipped one shared bearer per daemon, so a stolen token
//! couldn't be narrowed (every browser shared it) or revoked (restart the
//! daemon and everyone else loses access too). Per-device tokens fix both.

use std::net::IpAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use wisphive_daemon::state::{StateDb, WebDeviceRow};
use wisphive_protocol::DeviceId;

use crate::auth::{LoginThrottle, sha256_hex};

/// Authenticated web device context attached to a request by the security
/// middleware. Handlers and the ws_bridge read this from
/// `request.extensions()` to learn which device is driving.
#[derive(Clone, Debug)]
pub struct AuthedDevice {
    pub id: DeviceId,
    /// Operator-chosen device label, surfaced in audit logs + the (future)
    /// devices dashboard. Kept as an owned String so the extension is
    /// `'static` and can outlive the DB row.
    #[allow(dead_code)]
    pub name: String,
}

/// Remote IP for the current request. Populated by [`security_middleware`]
/// from `ConnectInfo<SocketAddr>` when axum is wired with
/// `into_make_service_with_connect_info`, or from a synthetic `ClientIp`
/// request extension in tests. Handlers read this via the
/// [`FromRequestParts`] impl below.
#[derive(Copy, Clone, Debug)]
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        if let Some(ip) = parts.extensions.get::<ClientIp>() {
            return Ok(*ip);
        }
        if let Some(ci) = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        {
            return Ok(ClientIp(ci.0.ip()));
        }
        // Fall back to loopback so local smoke tests and the oneshot
        // test harness keep working without a real TCP listener. In
        // production the middleware will have inserted ClientIp above,
        // so we never hit this branch.
        Ok(ClientIp(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
    }
}

/// Everything the security middleware + login handler need. Cheap to clone
/// (Arc internally).
#[derive(Clone)]
pub struct SecurityConfig {
    inner: Arc<SecurityConfigInner>,
}

struct SecurityConfigInner {
    allowed_origins: Vec<String>,
    allowed_hosts: Vec<String>,
    state_db: StateDb,
    throttle: LoginThrottle,
}

impl SecurityConfig {
    /// Build a security config.
    ///
    /// Seeds the host/origin allowlists for `127.0.0.1:<port>`,
    /// `localhost:<port>`, and — when `bind_host` is `0.0.0.0` or another
    /// non-loopback address — every additional hostname/IP that
    /// [`crate::tls::enumerate_lan_urls`] would advertise in the startup
    /// banner. That keeps the allowlist in lockstep with the URLs users are
    /// told to type on their phones: if we tell them the URL, we accept it.
    ///
    /// Dev mode also allows `http://localhost:5173` / `http://127.0.0.1:5173`
    /// (Vite's default dev port) as origins, because in dev the frontend is
    /// served cross-origin by Vite.
    ///
    /// `WISPHIVE_WEB_ALLOWED_ORIGINS` and `WISPHIVE_WEB_ALLOWED_HOSTS` env
    /// vars (comma-separated) extend the allowlists further — use for
    /// reverse-proxy setups that bind something the auto-LAN enumeration
    /// can't see.
    pub fn build(
        state_db: StateDb,
        bind_host: IpAddr,
        port: u16,
        dev_mode: bool,
    ) -> anyhow::Result<Self> {
        let mut allowed_origins = vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
            format!("https://127.0.0.1:{port}"),
            format!("https://localhost:{port}"),
        ];
        let mut allowed_hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];

        // Auto-LAN: mirror whatever enumerate_lan_urls advertises to the
        // user as valid phone URLs. Without this, `wisphive web --host 0.0.0.0`
        // would print `https://192.168.1.7:3100` and then reject it at the
        // Host-header check.
        for url in crate::tls::enumerate_lan_urls(bind_host, port) {
            if !allowed_origins.contains(&url) {
                allowed_origins.push(url.clone());
            }
            if let Some(host_port) = url.strip_prefix("https://") {
                let host_port = host_port.to_string();
                if !allowed_hosts.contains(&host_port) {
                    allowed_hosts.push(host_port);
                }
            }
        }

        if dev_mode {
            allowed_origins.push("http://localhost:5173".to_string());
            allowed_origins.push("http://127.0.0.1:5173".to_string());
        }

        if let Ok(extra) = std::env::var("WISPHIVE_WEB_ALLOWED_ORIGINS") {
            for o in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                allowed_origins.push(o.to_string());
            }
        }
        if let Ok(extra) = std::env::var("WISPHIVE_WEB_ALLOWED_HOSTS") {
            for h in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                allowed_hosts.push(h.to_string());
            }
        }

        Ok(Self {
            inner: Arc::new(SecurityConfigInner {
                allowed_origins,
                allowed_hosts,
                state_db,
                throttle: LoginThrottle::new(),
            }),
        })
    }

    /// Construct a config with explicit allowlists + an externally-owned DB.
    /// Used by tests so we can assert exact middleware behaviour without a
    /// live TCP listener.
    #[cfg(test)]
    pub fn for_test(
        allowed_origins: Vec<String>,
        allowed_hosts: Vec<String>,
        state_db: StateDb,
    ) -> Self {
        Self {
            inner: Arc::new(SecurityConfigInner {
                allowed_origins,
                allowed_hosts,
                state_db,
                throttle: LoginThrottle::new(),
            }),
        }
    }

    pub fn state_db(&self) -> &StateDb {
        &self.inner.state_db
    }

    pub fn throttle(&self) -> &LoginThrottle {
        &self.inner.throttle
    }

    fn check_host(&self, headers: &HeaderMap) -> bool {
        let Some(host) = headers.get("host").and_then(|h| h.to_str().ok()) else {
            // HTTP/1.1 requires a Host header; its absence is suspicious on
            // its own. Reject rather than silently allow.
            return false;
        };
        self.inner.allowed_hosts.iter().any(|h| h == host)
    }

    fn check_origin(&self, headers: &HeaderMap) -> bool {
        // Same-origin navigations from a browser do not send an Origin header
        // on top-level GETs — only cross-origin and fetch/WS requests do.
        // So "no Origin" is allowed; only a *mismatched* Origin is rejected.
        let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) else {
            return true;
        };
        self.inner.allowed_origins.iter().any(|o| o == origin)
    }

    /// Look up the presented token against `web_devices`. Returns the device
    /// row on a valid, non-revoked match.
    ///
    /// The presented token is hashed with SHA-256 before the DB lookup —
    /// the raw token never reaches the daemon crate, and `find_web_device_
    /// by_token_hash` filters `revoked_at IS NULL` server-side so a revoked
    /// token and a never-seen token are indistinguishable from here. That
    /// matches the handoff's "no info leak" rule: both fall through as
    /// `None` and the caller returns 401.
    async fn lookup_device_token(&self, presented_raw: &str) -> Option<WebDeviceRow> {
        let presented_hash = sha256_hex(presented_raw.as_bytes());
        match self
            .inner
            .state_db
            .find_web_device_by_token_hash(&presented_hash)
            .await
        {
            Ok(Some(row)) => Some(row),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "device token lookup failed");
                None
            }
        }
    }
}

/// Axum middleware — gates requests based on SecurityConfig. Returns 403 for
/// bad Host/Origin, 401 for missing/bad device token on protected paths.
///
/// On valid authentication, attaches an [`AuthedDevice`] extension to the
/// request so downstream handlers (and the ws_bridge) can learn which device
/// is driving without re-running the lookup.
pub async fn security_middleware(
    State(security): State<SecurityConfig>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    let uri = req.uri().clone();
    let path = uri.path();

    if !security.check_host(&headers) {
        tracing::warn!(
            ?path,
            host = ?headers.get("host"),
            "rejecting request: host not in allowlist"
        );
        return (StatusCode::FORBIDDEN, "host not allowed").into_response();
    }

    if !security.check_origin(&headers) {
        tracing::warn!(
            ?path,
            origin = ?headers.get("origin"),
            "rejecting request: origin not in allowlist"
        );
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    // Propagate ConnectInfo into a typed ClientIp extension so handlers can
    // pull it via one path regardless of whether they're under the real
    // axum::serve or a oneshot test request that seeded ClientIp directly.
    if req.extensions().get::<ClientIp>().is_none()
        && let Some(ci) = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        let ip = ClientIp(ci.0.ip());
        req.extensions_mut().insert(ip);
    }

    if path_requires_device_token(path) {
        let token = match extract_presented_token(&headers, uri.query()) {
            Some(t) => t,
            None => {
                tracing::warn!(?path, "rejecting request: missing device token");
                return (StatusCode::UNAUTHORIZED, "missing device token").into_response();
            }
        };
        let device = match security.lookup_device_token(&token).await {
            Some(row) => row,
            None => {
                tracing::warn!(?path, "rejecting request: unknown or revoked device token");
                return (StatusCode::UNAUTHORIZED, "invalid device token").into_response();
            }
        };

        // Keep the operator's device list meaningful: inline rather than
        // spawned, because the query is ~1 ms and spawning forces us to
        // move `security` into a 'static future (which the borrow checker
        // then keeps alive past the middleware's natural lifetime).
        let ip_str = req.extensions().get::<ClientIp>().map(|c| c.0.to_string());
        if let Err(e) = security
            .state_db()
            .touch_web_device(&device.id, ip_str.as_deref())
            .await
        {
            tracing::warn!(device_id = %device.id, error = %e, "touch_web_device failed");
        }

        req.extensions_mut().insert(AuthedDevice {
            id: DeviceId(device.id),
            name: device.name,
        });
    }

    next.run(req).await
}

/// Which paths require a device token. `/api/auth/login` is the credential
/// bootstrap — it's origin+host-gated and throttled, but obviously can't
/// require a device token since it's how you GET one.
///
/// `/api/web-token` is the retired per-process bootstrap route. We
/// deliberately exempt it from the gate even though the router no longer
/// handles it, so that the fallback (static handler / axum default) can
/// return a clean `404 Not Found` instead of a misleading `401
/// Unauthorized`. A 401 would tell clients "this path is a real thing, you
/// just can't see it" — exactly the opposite of the signal we want: that
/// the endpoint is gone and callers should move to `/api/auth/login`.
pub(crate) fn path_requires_device_token(path: &str) -> bool {
    if path == "/ws" {
        return true;
    }
    if path == "/api/auth/login" || path == "/api/web-token" {
        return false;
    }
    path.starts_with("/api/")
}

/// Read a presented token from `Authorization: Bearer <raw>` or `?token=<raw>`.
/// Browsers can't set Authorization on WebSocket upgrades, so the query
/// form is the primary path for `/ws`.
fn extract_presented_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(auth) = headers.get("authorization").and_then(|h| h.to_str().ok())
        && let Some(token) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
    {
        return Some(token.to_string());
    }
    if let Some(q) = query
        && let Some(token) = extract_query_param(q, "token")
    {
        return Some(token.to_string());
    }
    None
}

/// Extract `key=value` from a URL query string without pulling in a full URL
/// parser. Handles multiple keys and URL-encoded values sufficiently for our
/// use (tokens are base64url, which is already URL-safe).
fn extract_query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_requires_device_token_rules() {
        assert!(path_requires_device_token("/ws"));
        assert!(path_requires_device_token("/api/config"));
        assert!(path_requires_device_token("/api/devices"));
        assert!(!path_requires_device_token("/api/auth/login"));
        // Retired bootstrap route is exempt so the router's fallback
        // can 404 cleanly instead of the gate 401-ing first.
        assert!(!path_requires_device_token("/api/web-token"));
        assert!(!path_requires_device_token("/"));
        assert!(!path_requires_device_token("/index.html"));
        assert!(!path_requires_device_token("/assets/foo.js"));
    }

    #[test]
    fn extract_query_param_parses_simple_cases() {
        assert_eq!(extract_query_param("token=abc", "token"), Some("abc"));
        assert_eq!(
            extract_query_param("x=1&token=abc&y=2", "token"),
            Some("abc")
        );
        assert_eq!(extract_query_param("x=1&y=2", "token"), None);
    }

    #[test]
    fn extract_presented_token_prefers_header() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer header-token".parse().unwrap());
        assert_eq!(
            extract_presented_token(&headers, Some("token=query-token")).as_deref(),
            Some("header-token")
        );
    }

    #[test]
    fn extract_presented_token_falls_back_to_query() {
        let headers = HeaderMap::new();
        assert_eq!(
            extract_presented_token(&headers, Some("token=query-token")).as_deref(),
            Some("query-token")
        );
    }

    #[test]
    fn extract_presented_token_accepts_lowercase_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "bearer lc-token".parse().unwrap());
        assert_eq!(
            extract_presented_token(&headers, None).as_deref(),
            Some("lc-token")
        );
    }
}
