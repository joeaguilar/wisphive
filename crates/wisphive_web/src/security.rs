//! Security layer for the web UI.
//!
//! Gates the /ws and /api routes behind three checks:
//!
//! 1. **Host header allowlist** — defeats DNS rebinding. The daemon is bound
//!    to localhost (or 0.0.0.0 for LAN), but a rebound domain would send a
//!    non-localhost `Host` header, which we reject.
//! 2. **Origin header allowlist** — when present, must match our configured
//!    origins. Same-origin navigations to `/` don't include `Origin`, so we
//!    don't require it — but any cross-origin fetch (CSRF, WebSocket) will.
//! 3. **Bearer token** — per-process, 32 random bytes, base64url. Written
//!    0600 to `~/.wisphive/web.token`. Required on /ws (via `?token=` query)
//!    and /api/* (via `Authorization: Bearer` or `?token=` query).
//!
//! Together these defeat the CVSS ~9.0 vector where a malicious web page
//! opens `ws://127.0.0.1:3100/ws` and drives the daemon with full Tui
//! privileges.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use rand::RngCore;

/// Everything the security middleware needs. Cheap to clone (Arc internally).
#[derive(Clone)]
pub struct SecurityConfig {
    inner: Arc<SecurityConfigInner>,
}

struct SecurityConfigInner {
    token: String,
    allowed_origins: Vec<String>,
    allowed_hosts: Vec<String>,
}

impl SecurityConfig {
    /// Build a security config. Generates a fresh token, writes it 0600 to
    /// `<home_dir>/web.token`, and seeds the host/origin allowlists for
    /// `127.0.0.1:<port>` and `localhost:<port>`.
    ///
    /// Dev mode also allows `http://localhost:5173` / `http://127.0.0.1:5173`
    /// (Vite's default dev port) as origins, because in dev the frontend is
    /// served cross-origin by Vite.
    ///
    /// `WISPHIVE_WEB_ALLOWED_ORIGINS` and `WISPHIVE_WEB_ALLOWED_HOSTS` env
    /// vars (comma-separated) extend the allowlists — required when binding
    /// to `0.0.0.0` for LAN access, since we can't guess the client's IP.
    pub fn build(home_dir: &Path, port: u16, dev_mode: bool) -> anyhow::Result<Self> {
        let token = generate_token();
        write_token_file(home_dir, &token)?;

        let mut allowed_origins = vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ];
        let mut allowed_hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];

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
                token,
                allowed_origins,
                allowed_hosts,
            }),
        })
    }

    /// Construct a config with an explicit token. Used for tests so we can
    /// assert exactly which token the middleware expects.
    #[cfg(test)]
    pub fn for_test(
        token: String,
        allowed_origins: Vec<String>,
        allowed_hosts: Vec<String>,
    ) -> Self {
        Self {
            inner: Arc::new(SecurityConfigInner {
                token,
                allowed_origins,
                allowed_hosts,
            }),
        }
    }

    pub fn token(&self) -> &str {
        &self.inner.token
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

    fn check_token(&self, headers: &HeaderMap, query: Option<&str>) -> bool {
        // Try Authorization: Bearer <token>
        if let Some(auth) = headers.get("authorization").and_then(|h| h.to_str().ok())
            && let Some(token) = auth
                .strip_prefix("Bearer ")
                .or_else(|| auth.strip_prefix("bearer "))
            && constant_time_eq(token.as_bytes(), self.inner.token.as_bytes())
        {
            return true;
        }

        // Try ?token= query param. Browsers can't set Authorization on
        // WebSocket upgrades, so this is the primary path for /ws.
        if let Some(q) = query
            && let Some(token) = extract_query_param(q, "token")
            && constant_time_eq(token.as_bytes(), self.inner.token.as_bytes())
        {
            return true;
        }

        false
    }
}

/// Axum middleware — gates requests based on SecurityConfig. Returns 403 for
/// bad Host/Origin, 401 for missing/bad bearer token on protected paths.
pub async fn security_middleware(
    State(security): State<SecurityConfig>,
    req: Request<Body>,
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

    if path_requires_bearer(path) && !security.check_token(&headers, uri.query()) {
        tracing::warn!(?path, "rejecting request: missing or invalid bearer token");
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }

    next.run(req).await
}

/// Which paths require a bearer token. /api/web-token is the chicken-and-egg
/// bootstrap — it's origin+host-gated but not bearer-gated, so the frontend
/// can fetch the token on startup.
fn path_requires_bearer(path: &str) -> bool {
    if path == "/ws" {
        return true;
    }
    if path == "/api/web-token" {
        return false;
    }
    path.starts_with("/api/")
}

/// Constant-time byte comparison — the simple `==` operator leaks timing
/// information that can be amplified to recover tokens one byte at a time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn token_path(home_dir: &Path) -> PathBuf {
    home_dir.join("web.token")
}

#[cfg(unix)]
fn write_token_file(home_dir: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::create_dir_all(home_dir)?;
    let path = token_path(home_dir);

    // Remove first so we don't inherit wider permissions from a previous run
    // with a different mode.
    let _ = std::fs::remove_file(&path);

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(not(unix))]
fn write_token_file(home_dir: &Path, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(home_dir)?;
    std::fs::write(token_path(home_dir), format!("{token}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_produces_unique_base64url() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        // base64url of 32 bytes with no padding is 43 chars.
        assert_eq!(a.len(), 43);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
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
    fn path_requires_bearer_rules() {
        assert!(path_requires_bearer("/ws"));
        assert!(path_requires_bearer("/api/config"));
        assert!(!path_requires_bearer("/api/web-token"));
        assert!(!path_requires_bearer("/"));
        assert!(!path_requires_bearer("/index.html"));
        assert!(!path_requires_bearer("/assets/foo.js"));
    }

    #[cfg(unix)]
    #[test]
    fn write_token_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        write_token_file(tmp.path(), "abc").unwrap();
        let meta = std::fs::metadata(tmp.path().join("web.token")).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[cfg(unix)]
    #[test]
    fn write_token_file_overwrites_with_new_perms() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("web.token");
        // Pre-create with wide perms
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        write_token_file(tmp.path(), "new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "new");
    }
}
