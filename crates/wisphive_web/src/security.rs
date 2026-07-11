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
//!    `Authorization: Bearer <raw>`. `/ws` alone also accepts `?token=<raw>`
//!    (itr#494) because browsers cannot set custom headers on a WebSocket
//!    upgrade handshake; every ordinary `/api/*` HTTP request has no such
//!    constraint and rejects a query-string token even when it's otherwise
//!    valid, so a bearer can't leak via browser history, proxy/access logs,
//!    `Referer` propagation, screenshots, or copied links.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use url::Url;
use wisphive_daemon::state::{StateDb, WebAuthResult, WebDeviceRow};
use wisphive_protocol::DeviceId;

use crate::auth::{LoginThrottle, PeekThrottle, sha256_hex};
use crate::auth_profile::AuthPolicy;

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

/// The request's `Origin` header, parsed once by [`SecurityConfig::check_origin`]
/// in [`security_middleware`] and attached as a request extension for
/// downstream handlers (itr#317).
///
/// Before this existed, `check_origin` enforced the allowlist via a raw
/// string compare, and `get_auth_profile` in `lib.rs` separately re-parsed
/// the same `Origin` header with `Url::parse` to compute
/// `rp_id_for_origin`. Two independent parsers reading the same header
/// invite disagreement on malformed input; parsing once here and passing
/// the result through `Extension<ParsedOrigin>` removes that class of bug
/// and the duplicate work. Only inserted when the `Origin` header is
/// present *and* parses as a URL — absent for same-origin navigations
/// (which don't send `Origin` at all) and for the (practically
/// unreachable, since every allowlist entry is itself a well-formed URL)
/// case of a header that matched the allowlist string but failed to parse.
#[derive(Clone, Debug)]
pub struct ParsedOrigin(pub Url);

/// Outcome of [`SecurityConfig::check_origin`]: either the request is
/// rejected outright (403), or it's allowed and carries whatever `Origin`
/// was successfully parsed (if any) for the caller to attach as a request
/// extension.
enum OriginCheck {
    Rejected,
    Allowed(Option<ParsedOrigin>),
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
    /// Per-IP rate limit for the unauth read-only discovery endpoints
    /// (`/api/auth/status`, `/api/auth/profile`) — deliberately a separate
    /// budget from `throttle` above; see [`PeekThrottle`]'s docs (itr#317).
    peek_throttle: PeekThrottle,
    /// Active auth posture (itr#310). Cloned into the config so handlers
    /// (and the future WebAuthn machinery from #311) can branch on it
    /// without re-reading the profile from disk.
    auth_policy: AuthPolicy,
    /// TCP port the server is bound to. itr#311 needs this to derive the
    /// LocalLAN `rp_origin` (`https://localhost:<port>`) without parsing
    /// the request URL — `AuthPolicy::rp_id_for_origin` collapses every
    /// loopback host to RP ID `"localhost"`, so the matching `rp_origin`
    /// must be constructed from the configured port, not the
    /// (potentially `127.0.0.1`-typed) request URL.
    bind_port: u16,
    /// itr#256/#497: process-local, **bounded-TTL** cache of "is a web
    /// password currently set". `/api/auth/status` (an unauthenticated,
    /// unthrottled-by-token discovery endpoint) and the setup-required gate
    /// in [`security_middleware`] both used to run a SQLite `SELECT` on
    /// *every* request — a small-scale DoS surface and a free "is this
    /// instance set up yet" oracle for anyone who can reach the port.
    ///
    /// [`SecurityConfig::password_set`] serves from this flag without a DB
    /// round-trip **until [`SecurityConfigInner::password_set_valid_until_ms`]
    /// elapses** (one [`PASSWORD_SET_CACHE_TTL`] past the last DB
    /// confirmation). Once the deadline passes, the next call re-reads
    /// `get_web_password_hash` and either extends the deadline (still set)
    /// or flips the flag back to `false` (cleared out-of-process).
    /// `post_auth_set_password` in `lib.rs` also latches the flag directly
    /// and eagerly on a successful bootstrap via
    /// [`SecurityConfig::mark_password_set`], so the very next request —
    /// even from a different client — is a cache hit.
    ///
    /// **Why bounded-TTL rather than sticky-true (itr#497).** Earlier this
    /// was a "sticky true" flag that only ever transitioned `false -> true`.
    /// But `wisphive web reset-password` is a *separate* CLI process
    /// (`open_db()` in `wisphive_cli::commands::web`) that deletes the
    /// password row straight in `wisphive.db` with no signal to a running
    /// server. A sticky-true cache left `/api/auth/status` reporting
    /// `setup_required: false` forever after such a reset, so the SPA kept
    /// rendering Login (which then 401s, the hash being gone) and the
    /// unauthenticated setup endpoint stayed live but hidden — recoverable
    /// only by a full restart. The bounded TTL self-heals within a few
    /// seconds regardless of which process cleared the hash, cross-process,
    /// with no in-memory signal required. Device-token lookups always hit
    /// the DB fresh regardless (see
    /// [`SecurityConfig::lookup_device_token`]); this cache only fronts the
    /// setup-required discovery surface.
    password_set_cache: AtomicBool,
    /// Companion deadline for `password_set_cache`:
    /// [`SecurityConfigInner::base`]-relative milliseconds after which a
    /// cached `true` is stale and must be re-confirmed against the DB. Set
    /// to `now_ms + `[`PASSWORD_SET_CACHE_TTL`] on each DB confirmation.
    /// Only meaningful when `password_set_cache` is `true`; a stored `0`
    /// means "always stale" (used to force a re-check).
    password_set_valid_until_ms: AtomicU64,
    /// Monotonic base captured at construction. All cache timestamps are
    /// stored as milliseconds elapsed since this instant so they fit in an
    /// [`AtomicU64`] (an [`Instant`] can't live in an atomic directly).
    base: Instant,
}

/// How long [`SecurityConfig::password_set`] trusts a cached `true` before
/// re-confirming it against the DB. Short enough that a live
/// `wisphive web reset-password` self-heals within seconds (itr#497), long
/// enough that the steady-state authenticated case almost never pays the
/// SQLite round-trip.
const PASSWORD_SET_CACHE_TTL: Duration = Duration::from_secs(5);

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
        auth_policy: AuthPolicy,
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
                peek_throttle: PeekThrottle::new(),
                auth_policy,
                bind_port: port,
                password_set_cache: AtomicBool::new(false),
                password_set_valid_until_ms: AtomicU64::new(0),
                base: Instant::now(),
            }),
        })
    }

    /// Construct a config with explicit allowlists + an externally-owned DB.
    /// Used by tests so we can assert exact middleware behaviour without a
    /// live TCP listener. Defaults the [`AuthPolicy`] to the LocalLAN
    /// preset since most security tests are profile-agnostic.
    #[cfg(test)]
    pub fn for_test(
        allowed_origins: Vec<String>,
        allowed_hosts: Vec<String>,
        state_db: StateDb,
    ) -> Self {
        Self::for_test_with_policy(
            allowed_origins,
            allowed_hosts,
            state_db,
            crate::auth_profile::AuthProfile::LocalLAN.policy(),
        )
    }

    /// Like [`for_test`] but lets a caller pin a non-default policy when
    /// the test specifically asserts profile-driven behaviour.
    #[cfg(test)]
    pub fn for_test_with_policy(
        allowed_origins: Vec<String>,
        allowed_hosts: Vec<String>,
        state_db: StateDb,
        auth_policy: AuthPolicy,
    ) -> Self {
        Self {
            inner: Arc::new(SecurityConfigInner {
                allowed_origins,
                allowed_hosts,
                state_db,
                throttle: LoginThrottle::new(),
                peek_throttle: PeekThrottle::new(),
                auth_policy,
                // 3100 is the production default; tests don't actually
                // bind it so the value only feeds the rp_origin
                // derivation in itr#311's passkey handlers.
                bind_port: 3100,
                password_set_cache: AtomicBool::new(false),
                password_set_valid_until_ms: AtomicU64::new(0),
                base: Instant::now(),
            }),
        }
    }

    /// Port the server is bound to. Used by the itr#311 passkey
    /// handlers to derive a canonical LocalLAN `rp_origin` without
    /// parsing the request URL (which would let a `127.0.0.1` client
    /// bypass the loopback-collapse contract baked into `AuthPolicy`).
    pub fn bind_port(&self) -> u16 {
        self.inner.bind_port
    }

    /// Expose the frozen [`AuthPolicy`] so downstream handlers can branch
    /// on it (itr#310).
    #[allow(dead_code)]
    pub fn auth_policy(&self) -> &AuthPolicy {
        &self.inner.auth_policy
    }

    pub fn state_db(&self) -> &StateDb {
        &self.inner.state_db
    }

    pub fn throttle(&self) -> &LoginThrottle {
        &self.inner.throttle
    }

    /// The [`PeekThrottle`] for unauth read-only discovery endpoints —
    /// separate budget from [`Self::throttle`] (itr#317).
    pub fn peek_throttle(&self) -> &PeekThrottle {
        &self.inner.peek_throttle
    }

    /// Whether a web password is currently set (itr#256/#497).
    ///
    /// A bounded-TTL cache-aside over `get_web_password_hash`. When the
    /// [`SecurityConfigInner::password_set_cache`] flag is `true` **and** it
    /// was confirmed against the DB within [`PASSWORD_SET_CACHE_TTL`], this
    /// returns `true` immediately with no SQLite access — the steady-state
    /// authenticated case. Once that confirmation goes stale, the call
    /// re-reads the DB and either refreshes the confirmation timestamp
    /// (still set) or flips the flag back to `false` (the password was
    /// cleared out-of-process, e.g. by `wisphive web reset-password`), so a
    /// live reset self-heals within the TTL without a server restart.
    ///
    /// When the flag is `false` (fresh process, or genuinely unset) it
    /// falls back to the one SQLite round-trip on every call — the
    /// pre-setup window, where hitting the DB each time is expected — and
    /// latches the flag the moment a password appears. Used by both
    /// `get_auth_status` (`lib.rs`) and the setup-required gate in
    /// [`security_middleware`] so the two call sites share one cache
    /// instead of each probing SQLite independently.
    pub async fn password_set(&self) -> WebAuthResult<bool> {
        if self.inner.password_set_cache.load(Ordering::Relaxed) {
            let now_ms = self.now_ms();
            let valid_until = self
                .inner
                .password_set_valid_until_ms
                .load(Ordering::Relaxed);
            if now_ms < valid_until {
                // Fresh cached true — the common authenticated case, served
                // with no DB round-trip.
                return Ok(true);
            }
            // Deadline passed: re-confirm against the DB. A concurrent
            // `reset-password` may have cleared the row out-of-process.
            let is_set = self.inner.state_db.get_web_password_hash().await?.is_some();
            if is_set {
                self.inner.password_set_valid_until_ms.store(
                    now_ms.saturating_add(Self::cache_ttl_ms()),
                    Ordering::Relaxed,
                );
            } else {
                self.inner
                    .password_set_cache
                    .store(false, Ordering::Relaxed);
            }
            return Ok(is_set);
        }
        let is_set = self.inner.state_db.get_web_password_hash().await?.is_some();
        if is_set {
            // Store the deadline before the flag so a reader that observes
            // `true` also sees a fresh `valid_until`.
            self.inner.password_set_valid_until_ms.store(
                self.now_ms().saturating_add(Self::cache_ttl_ms()),
                Ordering::Relaxed,
            );
            self.inner.password_set_cache.store(true, Ordering::Relaxed);
        }
        Ok(is_set)
    }

    /// Latch the [`SecurityConfigInner::password_set_cache`] flag directly,
    /// without a DB round-trip. Called by `post_auth_set_password` in
    /// `lib.rs` right after a successful bootstrap write, so the very next
    /// request — even from a different client — is a cache hit instead of
    /// paying for the SQLite lookup [`Self::password_set`] would otherwise
    /// need to discover the same fact. Stamps the confirmation timestamp so
    /// the bounded-TTL window (itr#497) starts fresh from this write.
    pub fn mark_password_set(&self) {
        // Deadline before flag: a reader seeing `true` must also see a
        // fresh `valid_until`, else it would needlessly re-hit the DB.
        self.inner.password_set_valid_until_ms.store(
            self.now_ms().saturating_add(Self::cache_ttl_ms()),
            Ordering::Relaxed,
        );
        self.inner.password_set_cache.store(true, Ordering::Relaxed);
    }

    /// [`PASSWORD_SET_CACHE_TTL`] in milliseconds.
    fn cache_ttl_ms() -> u64 {
        PASSWORD_SET_CACHE_TTL.as_millis() as u64
    }

    /// Milliseconds elapsed since [`SecurityConfigInner::base`] — the clock
    /// backing the bounded-TTL `password_set` cache. Monotonic and cheap.
    fn now_ms(&self) -> u64 {
        self.inner.base.elapsed().as_millis() as u64
    }

    /// Force the `password_set` cache's deadline into the past so the next
    /// [`Self::password_set`] call treats a cached `true` as stale and
    /// re-reads the DB — lets tests exercise the bounded-TTL re-check
    /// (itr#497) without sleeping [`PASSWORD_SET_CACHE_TTL`]. Storing `0` is
    /// unconditionally stale because `now_ms >= 0` always holds.
    #[cfg(test)]
    fn expire_password_set_cache_for_test(&self) {
        self.inner
            .password_set_valid_until_ms
            .store(0, Ordering::Relaxed);
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

    /// Enforce the Origin allowlist and, on a pass, parse the header once
    /// so callers don't have to re-parse it downstream (itr#317; see
    /// [`ParsedOrigin`]).
    fn check_origin(&self, headers: &HeaderMap) -> OriginCheck {
        // Same-origin navigations from a browser do not send an Origin header
        // on top-level GETs — only cross-origin and fetch/WS requests do.
        // So "no Origin" is allowed; only a *mismatched* Origin is rejected.
        let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) else {
            return OriginCheck::Allowed(None);
        };
        if !self.inner.allowed_origins.iter().any(|o| o == origin) {
            return OriginCheck::Rejected;
        }
        // Parsed exactly once here. Every allowlist entry is itself a
        // well-formed URL (constructed via `format!("http(s)://host:port")`
        // in `build`), so in practice this always succeeds for a matching
        // Origin — but stay defensive and simply omit the extension rather
        // than reject the (already allowlisted) request if it doesn't.
        OriginCheck::Allowed(Url::parse(origin).ok().map(ParsedOrigin))
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

    match security.check_origin(&headers) {
        OriginCheck::Rejected => {
            tracing::warn!(
                ?path,
                origin = ?headers.get("origin"),
                "rejecting request: origin not in allowlist"
            );
            return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
        OriginCheck::Allowed(Some(parsed)) => {
            req.extensions_mut().insert(parsed);
        }
        OriginCheck::Allowed(None) => {}
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
    // `password_set` (itr#256/#497) is a bounded-TTL cache-aside: a password
    // observed set is served from memory for up to `PASSWORD_SET_CACHE_TTL`
    // before the DB is re-read, so a live `wisphive web reset-password` (a
    // separate process) self-heals this gate back to setup-required within
    // the TTL without a server restart. See
    // `SecurityConfigInner::password_set_cache`'s docs.
    let is_gated_api_path = path == "/ws" || path.starts_with("/api/");
    if is_gated_api_path && !path_bypasses_setup_gate(path) {
        match security.password_set().await {
            Ok(true) => {}
            Ok(false) => {
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
        let token = match extract_presented_token(&headers, uri.query(), path) {
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
        || path == "/api/auth/profile"
        || path == "/api/auth/set-password"
        || path == "/api/web-token"
    {
        return false;
    }
    // itr#311: passkey login is the bootstrap for an unauth caller —
    // exactly like password login. Both endpoints are throttled via
    // `LoginThrottle` instead of bearer-gated. Register routes stay
    // bearer-gated (see the falls-through-to-true below) because you
    // can't enroll a credential against a device you don't yet own.
    if path == "/api/auth/passkey/login/start" || path == "/api/auth/passkey/login/finish" {
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
    // `/api/auth/profile` is part of the same bootstrap-discovery surface
    // as `/api/auth/status` (itr#310): the SPA reads it to learn which
    // login flows to render *before* a password is set, so the setup
    // gate must not 503 it.
    path == "/api/auth/status" || path == "/api/auth/profile" || path == "/api/auth/set-password"
}

/// Read a presented token from `Authorization: Bearer <raw>` or, for `/ws`
/// only, `?token=<raw>`.
///
/// Browsers can't set `Authorization` on a WebSocket upgrade request, so the
/// query-string form exists solely as the `/ws` handshake's escape hatch
/// (itr#494). Every other route — all of `/api/*` — MUST present the token
/// via the `Authorization` header; a valid token riding in the query string
/// of an ordinary HTTP request would otherwise leak through browser
/// history, reverse-proxy/access logs, `Referer` propagation, screenshots,
/// and copy-pasted links. `path_query_token_allowed` is the single source
/// of truth for which paths get the query-string fallback — keep it in
/// sync with any new query-token-eligible route.
fn extract_presented_token(headers: &HeaderMap, query: Option<&str>, path: &str) -> Option<String> {
    if let Some(auth) = headers.get("authorization").and_then(|h| h.to_str().ok())
        && let Some(token) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
    {
        return Some(token.to_string());
    }
    if path_query_token_allowed(path)
        && let Some(q) = query
        && let Some(token) = extract_query_param(q, "token")
    {
        return Some(token.to_string());
    }
    None
}

/// Paths allowed to authenticate via `?token=` in addition to the
/// `Authorization` header. Deliberately narrow: only `/ws`, because
/// browsers cannot set custom headers on a WebSocket upgrade handshake.
/// Every ordinary `/api/*` HTTP request has no such constraint and must
/// present its bearer token via the `Authorization` header (itr#494).
fn path_query_token_allowed(path: &str) -> bool {
    path == "/ws"
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

// ---------------------------------------------------------------------------
// Loopback-IP → localhost redirect (sprint-1 wave-4 manual smoke)
// ---------------------------------------------------------------------------

/// Redirect non-API browser navigation away from IP-literal loopback hosts
/// (`127.0.0.1`, `[::1]`) to the canonical `localhost` form.
///
/// **Why this exists.** WebAuthn forbids IP-literal RP IDs at the browser
/// layer (§5.1.3 step 9). A user who pastes `https://127.0.0.1:<port>` into
/// the address bar would correctly see `can_enroll_passkey_on_this_origin:
/// false` from `/api/auth/profile` (the policy returns `None` for these
/// origins post-fix), but they would also miss the entire passkey flow
/// silently because the SPA would just hide the affordance with no
/// explanation. The redirect transparently bumps them to `localhost` where
/// passkey enrollment actually works.
///
/// **Scope.**
/// - Only applies to non-`/api/*` non-`/ws` paths — operators and scripts
///   hitting the daemon's API directly via `127.0.0.1` should continue to
///   reach the API without redirection.
/// - Redirect is `308 Permanent Redirect` (preserves the request method;
///   POST stays POST under future-extensions, though in practice only GET
///   navigation reaches this layer because every other non-API request
///   has already been routed away above us).
/// - Scheme is taken from the URI (HTTP/2) or hardcoded to `https` (the
///   daemon's production posture). Anyone who wedges the daemon onto plain
///   HTTP under a loopback IP is in an unsupported configuration; the
///   `https` default surfaces that as a redirect-to-HTTPS hint.
/// - Host extraction uses the same Uri-authority-then-Host-header order
///   the auth-profile handler does (HTTP/2 puts the authority in the URI,
///   HTTP/1.1 puts it in the Host header).
pub async fn loopback_ip_redirect(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    // API and WebSocket paths bypass the redirect — they're for tooling
    // that doesn't render UI and may legitimately want to reach the
    // daemon via its bound IP.
    if path.starts_with("/api/") || path == "/ws" {
        return next.run(request).await;
    }

    // Resolve the host string the request arrived under. Same order as
    // `lib.rs::origin_can_enroll_passkey`: URI authority first (HTTP/2),
    // Host header second (HTTP/1.1).
    let host = request
        .uri()
        .authority()
        .map(|a| a.as_str().to_string())
        .or_else(|| {
            request
                .headers()
                .get("host")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string())
        });
    let Some(host) = host else {
        return next.run(request).await;
    };

    if !is_loopback_ip_host(&host) {
        return next.run(request).await;
    }

    // Build the redirect target — same path + query, but `localhost` swapped
    // for the IP literal. Port (if present) is preserved.
    let port = host_port(&host);
    let target_host = match port {
        Some(p) => format!("localhost:{p}"),
        None => "localhost".to_string(),
    };
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let location = format!("https://{target_host}{path_and_query}");

    let location_header = match HeaderValue::from_str(&location) {
        Ok(v) => v,
        Err(_) => return next.run(request).await,
    };

    let mut response = Response::new(Body::empty());
    *response.status_mut() = axum::http::StatusCode::PERMANENT_REDIRECT; // 308
    response.headers_mut().insert("location", location_header);
    response
}

/// Returns true when the host string (e.g. `127.0.0.1:3100`, `[::1]:3100`,
/// `localhost`) is an IPv4 or IPv6 loopback literal. DNS names (including
/// `localhost`) return false.
fn is_loopback_ip_host(host: &str) -> bool {
    let host_only = strip_port(host);
    if let Ok(ipv4) = host_only.parse::<std::net::Ipv4Addr>() {
        return ipv4.is_loopback();
    }
    // IPv6 host strings in URL form are wrapped in brackets: `[::1]:3100`.
    // After `strip_port`, the brackets are still there for the IPv6 case;
    // peel them before parsing.
    let bare = host_only.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ipv6) = bare.parse::<std::net::Ipv6Addr>() {
        return ipv6.is_loopback();
    }
    false
}

/// Strip the `:port` suffix from a host string, leaving the host portion.
/// Handles bracketed IPv6 (`[::1]:3100` → `[::1]`) and bare hostnames
/// (`localhost:3100` → `localhost`).
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        // IPv6 form. Port (if any) follows the closing bracket.
        if let Some(close) = host.find(']') {
            // Include the closing bracket so callers can still recognise
            // the bracketed IPv6 form; `is_loopback_ip_host` strips them
            // before parsing.
            &host[..=close]
        } else {
            host
        }
    } else if let Some(colon) = host.rfind(':') {
        &host[..colon]
    } else {
        host
    }
}

/// Extract the port portion from a host string, or `None` if no port is
/// present. Handles both `localhost:3100` and `[::1]:3100`.
fn host_port(host: &str) -> Option<&str> {
    if host.starts_with('[') {
        // IPv6 form: port follows the closing bracket.
        let close = host.find(']')?;
        let after = &host[close + 1..];
        after.strip_prefix(':')
    } else {
        let colon = host.rfind(':')?;
        Some(&host[colon + 1..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
        // itr#310: profile discovery is also public — same reason as
        // `/api/auth/status`, the SPA needs it before the user has a
        // token.
        assert!(!path_requires_device_token("/api/auth/profile"));
        // Retired bootstrap route is exempt so the router's fallback
        // can 404 cleanly instead of the gate 401-ing first.
        assert!(!path_requires_device_token("/api/web-token"));
        // itr#311: passkey login is the bootstrap for an unauth caller
        // — same logic as password /api/auth/login. Register routes
        // remain bearer-gated (a device must exist before passkey
        // enrollment).
        assert!(!path_requires_device_token("/api/auth/passkey/login/start"));
        assert!(!path_requires_device_token(
            "/api/auth/passkey/login/finish"
        ));
        assert!(path_requires_device_token(
            "/api/auth/passkey/register/start"
        ));
        assert!(path_requires_device_token(
            "/api/auth/passkey/register/finish"
        ));
        assert!(!path_requires_device_token("/"));
        assert!(!path_requires_device_token("/index.html"));
        assert!(!path_requires_device_token("/assets/foo.js"));
    }

    #[test]
    fn path_bypasses_setup_gate_rules() {
        assert!(path_bypasses_setup_gate("/api/auth/status"));
        assert!(path_bypasses_setup_gate("/api/auth/set-password"));
        // itr#310: profile bypasses the setup gate too — the SPA reads
        // it on the onboarding page (no password yet) to decide whether
        // to even offer the "enroll passkey" affordance.
        assert!(path_bypasses_setup_gate("/api/auth/profile"));
        assert!(!path_bypasses_setup_gate("/api/auth/login"));
        assert!(!path_bypasses_setup_gate("/api/config"));
        assert!(!path_bypasses_setup_gate("/api/devices"));
        assert!(!path_bypasses_setup_gate("/ws"));
        // itr#311: passkey routes are NOT exempt from the setup gate —
        // the operator must set a password before any passkey workflow
        // becomes reachable.
        assert!(!path_bypasses_setup_gate("/api/auth/passkey/login/start"));
        assert!(!path_bypasses_setup_gate(
            "/api/auth/passkey/register/start"
        ));
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
            extract_presented_token(&headers, Some("token=query-token"), "/ws").as_deref(),
            Some("header-token")
        );
    }

    #[test]
    fn extract_presented_token_falls_back_to_query_on_ws() {
        let headers = HeaderMap::new();
        assert_eq!(
            extract_presented_token(&headers, Some("token=query-token"), "/ws").as_deref(),
            Some("query-token")
        );
    }

    /// itr#494: the query-string fallback is scoped to `/ws` only. An
    /// ordinary `/api/*` request presenting a token via `?token=` (and no
    /// `Authorization` header) must NOT authenticate, even though the same
    /// query string would work against `/ws`.
    #[test]
    fn extract_presented_token_rejects_query_on_api_paths() {
        let headers = HeaderMap::new();
        assert_eq!(
            extract_presented_token(&headers, Some("token=query-token"), "/api/config"),
            None
        );
    }

    #[test]
    fn path_query_token_allowed_scoped_to_ws() {
        assert!(path_query_token_allowed("/ws"));
        assert!(!path_query_token_allowed("/api/config"));
        assert!(!path_query_token_allowed("/api/devices"));
        assert!(!path_query_token_allowed("/"));
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
            extract_presented_token(&headers, None, "/api/config").as_deref(),
            Some("lc-token")
        );
    }

    // ── Loopback-IP redirect helpers ───────────────────────────────────

    #[test]
    fn is_loopback_ip_host_recognises_ipv4_loopback() {
        assert!(is_loopback_ip_host("127.0.0.1"));
        assert!(is_loopback_ip_host("127.0.0.1:3100"));
        assert!(is_loopback_ip_host("127.255.255.254"));
        // Non-loopback IPv4 stays false.
        assert!(!is_loopback_ip_host("192.168.1.42"));
        assert!(!is_loopback_ip_host("192.168.1.42:8443"));
    }

    #[test]
    fn is_loopback_ip_host_recognises_ipv6_loopback() {
        assert!(is_loopback_ip_host("[::1]"));
        assert!(is_loopback_ip_host("[::1]:3100"));
        // Non-loopback IPv6 stays false.
        assert!(!is_loopback_ip_host("[2001:db8::1]:8443"));
    }

    #[test]
    fn is_loopback_ip_host_returns_false_for_dns_names() {
        // The whole point of the redirect is to bump IP literals to a
        // DNS name. `localhost` itself must NOT trigger the redirect.
        assert!(!is_loopback_ip_host("localhost"));
        assert!(!is_loopback_ip_host("localhost:3100"));
        assert!(!is_loopback_ip_host("wisphive.example.com"));
        assert!(!is_loopback_ip_host("wisphive.example.com:443"));
    }

    #[test]
    fn host_port_parses_both_forms() {
        assert_eq!(host_port("localhost:3100"), Some("3100"));
        assert_eq!(host_port("127.0.0.1:8443"), Some("8443"));
        assert_eq!(host_port("[::1]:3100"), Some("3100"));
        assert_eq!(host_port("localhost"), None);
        assert_eq!(host_port("[::1]"), None);
    }

    #[test]
    fn strip_port_handles_ipv6_brackets() {
        assert_eq!(strip_port("localhost:3100"), "localhost");
        assert_eq!(strip_port("127.0.0.1:3100"), "127.0.0.1");
        assert_eq!(strip_port("[::1]:3100"), "[::1]");
        assert_eq!(strip_port("[::1]"), "[::1]");
        assert_eq!(strip_port("localhost"), "localhost");
    }

    // ── password_set cache (itr#256) ────────────────────────────────────

    #[tokio::test]
    async fn password_set_reflects_db_before_any_write() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        assert!(!security.password_set().await.unwrap());
    }

    /// The load-bearing proof of the fast path: *within* the TTL, after the
    /// cache has observed `true` once, it must keep reporting `true` even
    /// when the underlying DB row is wiped out from under it — that can only
    /// happen if `password_set()` is served from the cached flag, not by
    /// re-querying SQLite on every call. (The test completes in
    /// milliseconds, far inside [`PASSWORD_SET_CACHE_TTL`], so the deadline
    /// has not yet elapsed — the stale-path re-check is covered separately
    /// by `password_set_reverts_to_false_after_ttl_when_db_reset`.)
    #[tokio::test]
    async fn password_set_caches_true_without_further_db_reads() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        security
            .state_db()
            .try_set_initial_web_password("fake-hash")
            .await
            .unwrap();

        // First call discovers the password via the one-time DB fallback
        // and latches the cache.
        assert!(security.password_set().await.unwrap());

        // Wipe the password row directly (mirrors what `wisphive web
        // reset-password` does out-of-process). If `password_set()` were
        // still hitting SQLite, this call would now observe `false`.
        security.state_db().reset_web_password().await.unwrap();
        assert!(
            security.password_set().await.unwrap(),
            "cached true must survive a DB-level password wipe within the TTL — \
             proves no SQLite round-trip happens on the fresh-cache fast path"
        );
    }

    /// itr#497: the bounded-TTL invalidation. `wisphive web reset-password`
    /// runs in a *separate* CLI process and deletes the password row
    /// straight in `wisphive.db` with no signal to a running server. Once
    /// the cached `true`'s deadline elapses, `password_set()` must re-read
    /// the DB, observe the wipe, and flip back to `false` — so a live reset
    /// self-heals `/api/auth/status` (which reads this) into setup-required
    /// mode without a server restart.
    #[tokio::test]
    async fn password_set_reverts_to_false_after_ttl_when_db_reset() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        security
            .state_db()
            .try_set_initial_web_password("fake-hash")
            .await
            .unwrap();

        // Latch the cache to true (mirrors a running server that has served
        // an authenticated session).
        assert!(security.password_set().await.unwrap());

        // Out-of-process reset: the password row is deleted with no
        // in-memory signal to this server.
        security.state_db().reset_web_password().await.unwrap();

        // Still true while the cache deadline hasn't elapsed.
        assert!(
            security.password_set().await.unwrap(),
            "cache must stay true until its TTL deadline passes"
        );

        // Simulate the TTL deadline passing (no real sleep).
        security.expire_password_set_cache_for_test();

        // Now the stale-path re-check runs, sees no password, and flips.
        assert!(
            !security.password_set().await.unwrap(),
            "after the TTL elapses the cache must re-read the DB and revert to \
             false so /api/auth/status reports setup-required again"
        );

        // And it stays false on the next call — the flag itself is now
        // false, so we're back on the pre-setup DB-every-call path.
        assert!(!security.password_set().await.unwrap());
    }

    /// A reset followed by a fresh bootstrap must re-latch: after the cache
    /// reverts to false and a new password is written, `password_set()`
    /// observes it and caches true again (the full self-heal cycle).
    #[tokio::test]
    async fn password_set_relatches_after_reset_then_new_password() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        security
            .state_db()
            .try_set_initial_web_password("hash-one")
            .await
            .unwrap();
        assert!(security.password_set().await.unwrap());

        security.state_db().reset_web_password().await.unwrap();
        security.expire_password_set_cache_for_test();
        assert!(!security.password_set().await.unwrap());

        // A new bootstrap writes a fresh password; the cache re-latches.
        security
            .state_db()
            .try_set_initial_web_password("hash-two")
            .await
            .unwrap();
        assert!(security.password_set().await.unwrap());
    }

    /// Mirrors what `post_auth_set_password` in `lib.rs` does immediately
    /// after a successful bootstrap write: latch the cache directly with
    /// no DB read at all, so the very next request is a hit.
    #[tokio::test]
    async fn mark_password_set_latches_cache_without_db_write() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        // No password persisted anywhere in the DB.
        security.mark_password_set();
        assert!(security.password_set().await.unwrap());
    }

    // ── ParsedOrigin (itr#317) ──────────────────────────────────────────

    async fn test_security_config(
        allowed_origins: Vec<String>,
    ) -> (tempfile::TempDir, SecurityConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wisphive.db");
        let db = StateDb::open(db_path.to_string_lossy().as_ref())
            .await
            .unwrap();
        let security =
            SecurityConfig::for_test(allowed_origins, vec!["localhost:3100".to_string()], db);
        (tmp, security)
    }

    #[tokio::test]
    async fn check_origin_parses_allowlisted_origin() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://localhost:3100".parse().unwrap());
        match security.check_origin(&headers) {
            OriginCheck::Allowed(Some(ParsedOrigin(url))) => {
                assert_eq!(url.as_str(), "https://localhost:3100/");
            }
            OriginCheck::Allowed(None) => panic!("expected a parsed origin, got None"),
            OriginCheck::Rejected => panic!("allowlisted origin must not be rejected"),
        }
    }

    #[tokio::test]
    async fn check_origin_allows_none_when_origin_absent() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        let headers = HeaderMap::new();
        assert!(matches!(
            security.check_origin(&headers),
            OriginCheck::Allowed(None)
        ));
    }

    #[tokio::test]
    async fn check_origin_rejects_unlisted_origin() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;
        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://evil.example".parse().unwrap());
        assert!(matches!(
            security.check_origin(&headers),
            OriginCheck::Rejected
        ));
    }

    /// End-to-end through the real middleware (not just `check_origin`
    /// directly): a request with an allowlisted `Origin` header should
    /// have `ParsedOrigin` attached to its extensions and readable by a
    /// downstream handler.
    #[tokio::test]
    async fn security_middleware_populates_parsed_origin_when_present() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;

        async fn probe(parsed: Option<axum::Extension<ParsedOrigin>>) -> String {
            match parsed {
                Some(axum::Extension(ParsedOrigin(url))) => url.to_string(),
                None => "absent".to_string(),
            }
        }

        let app = axum::Router::new()
            .route("/probe", axum::routing::get(probe))
            .layer(axum::middleware::from_fn_with_state(
                security,
                security_middleware,
            ));

        let request = Request::builder()
            .method("GET")
            .uri("/probe")
            .header("host", "localhost:3100")
            .header("origin", "https://localhost:3100")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(request).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"https://localhost:3100/");
    }

    /// Companion: no `Origin` header (the common same-origin-GET case) must
    /// leave `ParsedOrigin` absent rather than the handler observing a
    /// stale/empty value.
    #[tokio::test]
    async fn security_middleware_leaves_parsed_origin_absent_when_origin_missing() {
        let (_tmp, security) =
            test_security_config(vec!["https://localhost:3100".to_string()]).await;

        async fn probe(parsed: Option<axum::Extension<ParsedOrigin>>) -> String {
            match parsed {
                Some(axum::Extension(ParsedOrigin(url))) => url.to_string(),
                None => "absent".to_string(),
            }
        }

        let app = axum::Router::new()
            .route("/probe", axum::routing::get(probe))
            .layer(axum::middleware::from_fn_with_state(
                security,
                security_middleware,
            ));

        let request = Request::builder()
            .method("GET")
            .uri("/probe")
            .header("host", "localhost:3100")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(request).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"absent");
    }
}
