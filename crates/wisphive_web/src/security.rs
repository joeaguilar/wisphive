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
use axum::http::{HeaderMap, StatusCode, Uri};
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

    /// Resolve the effective host for this request and compare against
    /// [`SecurityConfigInner::allowed_hosts`].
    ///
    /// HTTP/1.1 carries the host in the `Host:` header. HTTP/2 (which
    /// axum_server negotiates by default once TLS is on via ALPN) drops
    /// `Host` on the wire entirely and puts the authority on the `:authority`
    /// pseudo-header. hyper surfaces that through `request.uri().authority()`
    /// but does NOT synthesize a `Host:` header for handlers to read.
    ///
    /// Before this fix we only looked at the `Host:` header and would 403
    /// every h2 request including every browser on a modern Chrome / Safari
    /// — a nasty latent bug that stayed hidden while the server was plain
    /// HTTP (no ALPN → no h2 → Host always present). The TLS swap in
    /// itr#214 flipped it on.
    ///
    /// Disagreement policy: RFC 9113 §8.3.1 requires `Host` and `:authority`
    /// to match exactly when both appear. hyper may or may not enforce this
    /// upstream depending on version / h2 settings (the two reviewers of
    /// itr#214 disagreed on its current behaviour), so we enforce it here
    /// too. A client that wants to pass a privileged Host: while steering
    /// TLS/SNI via a different :authority would otherwise have a narrow
    /// bypass window.
    fn check_host(&self, headers: &HeaderMap, uri: &Uri) -> bool {
        let header_host = headers.get("host").and_then(|h| h.to_str().ok());
        let authority = uri.authority().map(axum::http::uri::Authority::as_str);
        let candidate = match (header_host, authority) {
            (Some(h), Some(a)) if h != a => return false,
            (Some(h), _) => h,
            (None, Some(a)) => a,
            (None, None) => return false,
        };
        self.inner.allowed_hosts.iter().any(|h| h == candidate)
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

    if !security.check_host(&headers, &uri) {
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

    // Setup-required gate. If no web password has been persisted yet, any
    // `/api/*` or `/ws` request (except the discovery endpoint) returns
    // 503 — the SPA is expected to have hit `/api/auth/status` first and
    // routed the user to a setup page rather than a login form. Reaching
    // this gate with password_set=false means the client got confused, so
    // a hard error is correct.
    //
    // Doing a DB lookup on every gated request is cheap (SQLite, same pool
    // the rest of the handler uses, ~microseconds). If it ever shows up on
    // a profile, replace with an AtomicBool cached in `SecurityConfig` that
    // itr#215's `/api/auth/setup` handler flips to `password_set = true`
    // on successful bootstrap.
    let is_gated_api_path = path == "/ws" || path.starts_with("/api/");
    if is_gated_api_path && !path_bypasses_setup_gate(path) {
        match security.state_db().get_web_password_hash().await {
            Ok(Some(_)) => {}
            Ok(None) => {
                tracing::warn!(?path, "rejecting request: setup-required (no web password)");
                return setup_required_response();
            }
            Err(e) => {
                tracing::warn!(error = %e, "setup-required probe failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
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
/// require a device token since it's how you GET one. `/api/auth/status` is
/// the setup-discovery surface hit by the frontend before it knows whether
/// to show Login vs Setup; it intentionally leaks whether a password has
/// been set, so guarding it with a token would be silly.
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
    if path == "/api/auth/login"
        || path == "/api/auth/status"
        || path == "/api/auth/set-password"
        || path == "/api/web-token"
    {
        return false;
    }
    path.starts_with("/api/")
}

/// Canonical 503 response for setup-required mode. Body is JSON so the SPA
/// can machine-detect the case without string-matching on the text body;
/// `error` is a stable discriminant while `message` is for humans reading
/// a network-tab response.
fn setup_required_response() -> Response {
    let body = serde_json::json!({
        "error": "setup_required",
        "message": "Web UI has no password set; complete setup first."
    })
    .to_string();
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

/// Whether `path` is exempt from the setup-required gate.
///
/// In setup-required mode (no web password set yet), every `/api/*` and
/// `/ws` request must be refused with 503 — *except* the bootstrap
/// discovery surface. `/api/auth/status` is the discovery probe; the SPA
/// hits it before it knows there's a password and needs an honest answer
/// to pick the setup-vs-login branch. `/api/auth/set-password` is the
/// onboarding POST itself (itr#268) — it has no token to present and can
/// only succeed in setup mode (internally 409s once a password exists).
fn path_bypasses_setup_gate(path: &str) -> bool {
    path == "/api/auth/status" || path == "/api/auth/set-password"
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
        assert!(path_requires_device_token("/api/me"));
        assert!(!path_requires_device_token("/api/auth/login"));
        // Setup-discovery endpoint is public: the SPA hits it before it
        // knows whether to show Login or Setup.
        assert!(!path_requires_device_token("/api/auth/status"));
        // Retired bootstrap route is exempt so the router's fallback
        // can 404 cleanly instead of the gate 401-ing first.
        assert!(!path_requires_device_token("/api/web-token"));
        assert!(!path_requires_device_token("/"));
        assert!(!path_requires_device_token("/index.html"));
        assert!(!path_requires_device_token("/assets/foo.js"));
    }

    #[test]
    fn path_bypasses_setup_gate_rules() {
        assert!(path_bypasses_setup_gate("/api/auth/status"));
        assert!(path_bypasses_setup_gate("/api/auth/set-password"));
        assert!(!path_bypasses_setup_gate("/api/auth/login"));
        assert!(!path_bypasses_setup_gate("/api/config"));
        assert!(!path_bypasses_setup_gate("/api/devices"));
        assert!(!path_bypasses_setup_gate("/ws"));
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

    /// HTTP/2 drops the `Host:` header on the wire — the authority lives on
    /// the `:authority` pseudo-header, surfaced via `request.uri().authority()`.
    /// `check_host` MUST accept that path or every browser on a modern TLS
    /// stack gets 403'd once we flip on `axum_server` (which negotiates h2
    /// via ALPN by default).
    #[tokio::test]
    async fn check_host_accepts_http2_authority_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wisphive.db");
        let db = StateDb::open(db_path.to_string_lossy().as_ref())
            .await
            .unwrap();
        let security = SecurityConfig::for_test(
            vec!["https://127.0.0.1:3199".to_string()],
            vec!["127.0.0.1:3199".to_string()],
            db,
        );

        // HTTP/2-style: no Host header, authority baked into the URI. A real
        // hyper request for an h2 upstream gives us exactly this shape.
        let headers = HeaderMap::new();
        let uri: Uri = "https://127.0.0.1:3199/api/auth/status".parse().unwrap();
        assert!(security.check_host(&headers, &uri));

        // HTTP/1.1-style: Host header present, URI is path-only.
        let mut h11_headers = HeaderMap::new();
        h11_headers.insert("host", "127.0.0.1:3199".parse().unwrap());
        let h11_uri: Uri = "/api/auth/status".parse().unwrap();
        assert!(security.check_host(&h11_headers, &h11_uri));

        // Neither header nor authority: rejected.
        let empty_headers = HeaderMap::new();
        let empty_uri: Uri = "/api/auth/status".parse().unwrap();
        assert!(!security.check_host(&empty_headers, &empty_uri));

        // Mismatched authority: rejected (sanity — :authority can't be a
        // rebind escape hatch).
        let evil_uri: Uri = "https://evil.example/api/auth/status".parse().unwrap();
        assert!(!security.check_host(&HeaderMap::new(), &evil_uri));

        // Host/:authority disagreement MUST be rejected even when one of
        // the two is in the allowlist. This closes a narrow bypass where a
        // client could steer TLS/SNI to an :authority the server trusts
        // while presenting a Host the handler would gate on. RFC 9113 §8.3.1
        // mandates exact match; we enforce it here defensively.
        let mut mixed = HeaderMap::new();
        mixed.insert("host", "127.0.0.1:3199".parse().unwrap());
        let mismatched_uri: Uri = "https://evil.example:3199/api/auth/status".parse().unwrap();
        assert!(
            !security.check_host(&mixed, &mismatched_uri),
            "disagreement between Host header and :authority must 403"
        );
        // Inverse: allowlisted :authority with a Host that says otherwise
        // is equally rejected.
        let mut evil_host = HeaderMap::new();
        evil_host.insert("host", "evil.example:3199".parse().unwrap());
        let good_uri: Uri = "https://127.0.0.1:3199/api/auth/status".parse().unwrap();
        assert!(!security.check_host(&evil_host, &good_uri));

        // Agreement on the allowlisted host: accepted.
        let mut matching = HeaderMap::new();
        matching.insert("host", "127.0.0.1:3199".parse().unwrap());
        let matching_uri: Uri = "https://127.0.0.1:3199/api/auth/status".parse().unwrap();
        assert!(security.check_host(&matching, &matching_uri));
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
