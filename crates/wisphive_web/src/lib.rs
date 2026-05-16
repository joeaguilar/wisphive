pub mod auth;
pub mod auth_profile;
mod reauth_ipc;
mod security;
pub mod tls;
mod ws_bridge;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::WebSocket;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use rust_embed::RustEmbed;
use security::{AuthedDevice, ClientIp, SecurityConfig, security_middleware};
use serde::{Deserialize, Serialize};
use tracing::info;
use wisphive_daemon::state::StateDb;

pub use auth_profile::{AuthPolicy, AuthProfile, RpId, UvRequirement};
// Re-export the WebAuthn `Url` type so downstream crates (notably the CLI)
// can build `AuthProfile::Enterprise { rp_origin, .. }` without pulling
// in `webauthn-rs` or `url` themselves.
pub use webauthn_rs::prelude::Url;

/// Hard deadline on Argon2 verify. itr#245: without it, a stalled verify
/// pins a per-IP throttle slot for the full [`STALE_IN_FLIGHT_AGE`] (5 min)
/// and self-DoSes the originating IP. 5 s is generous — our Argon2 params
/// (m=19_456 KiB, t=2, p=1) complete in ~50 ms on a laptop; anything past
/// this is a stalled worker thread, not slow hardware.
///
/// [`STALE_IN_FLIGHT_AGE`]: auth::LoginThrottle
const VERIFY_DEADLINE: Duration = Duration::from_secs(5);

/// Shared server state.
#[derive(Clone)]
struct AppState {
    socket_path: PathBuf,
    config_path: PathBuf,
    security: SecurityConfig,
    /// Frozen auth/security posture (itr#310). Threaded into `AppState` so
    /// downstream handlers can branch on `AuthPolicy` without re-deriving
    /// it from the profile enum on every request.
    auth_policy: AuthPolicy,
}

/// Embedded frontend assets (built by Vite into frontend/dist/).
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

/// Serve an embedded static file, falling back to index.html for SPA routing.
///
/// Intentionally does NOT serve the SPA shell for `/api/*` or `/ws` paths:
/// those namespaces are API surface. An unregistered API path falling
/// through to index.html would hand HTML to an API client (confusing) and,
/// worse, would mask a 404 for a route the caller expected to exist (e.g.
/// the retired `/api/web-token`, which acceptance for itr#213 requires
/// resolve as 404, not 200-with-HTML).
async fn static_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
            file.data.to_vec(),
        )
            .into_response();
    }

    let request_path = uri.path();
    if request_path.starts_with("/api/") || request_path == "/ws" {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }

    if let Some(file) = FrontendAssets::get("index.html") {
        Html(std::str::from_utf8(&file.data).unwrap_or("").to_string()).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "not found").into_response()
    }
}

/// WebSocket upgrade handler — bridges browser ↔ daemon. The `AuthedDevice`
/// extractor gates the handler: if the security middleware didn't attach one,
/// axum returns 401 before we get here, so reaching this function means the
/// device is verified and non-revoked.
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Extension(device): axum::Extension<AuthedDevice>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state.socket_path, device))
}

async fn handle_ws(ws: WebSocket, socket_path: PathBuf, device: AuthedDevice) {
    if let Err(e) = ws_bridge::bridge(ws, &socket_path, device).await {
        tracing::warn!("WebSocket bridge error: {e}");
    }
}

/// GET /api/config — read config.json
async fn get_config(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    match std::fs::read_to_string(&state.config_path) {
        Ok(content) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            content,
        )
            .into_response(),
        Err(_) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            "{}".to_string(),
        )
            .into_response(),
    }
}

/// PUT /api/config — write config.json
async fn put_config(
    axum::extract::State(state): axum::extract::State<AppState>,
    body: String,
) -> Response {
    // Validate JSON
    if serde_json::from_str::<serde_json::Value>(&body).is_err() {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid JSON").into_response();
    }
    match std::fs::write(&state.config_path, &body) {
        Ok(_) => (axum::http::StatusCode::OK, "saved").into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("write failed: {e}"),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    password: String,
    /// Friendly label stored alongside the device token so the operator can
    /// tell devices apart on the dashboard (e.g. "iphone", "laptop-work").
    /// Optional; falls back to a truncated device id.
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    device_id: String,
    /// The raw bearer token — the client MUST save this; it's not retrievable
    /// from the server afterwards. The server stores only `sha256(raw)`.
    token: String,
}

/// POST /api/auth/login — the credential bootstrap endpoint.
///
/// Flow:
/// 1. Reserve a per-IP in-flight slot via [`LoginThrottle::try_begin_attempt`].
///    On `Err`, return `429 Too Many Requests` with a `Retry-After` header.
/// 2. Fetch the stored Argon2 hash. If none is set (first-run host), treat
///    it as an invalid-credentials failure rather than leaking the state.
/// 3. Verify the presented password against the hash inside a
///    [`tokio::task::spawn_blocking`] wrapped in `tokio::time::timeout`
///    ([`VERIFY_DEADLINE`]). A stalled verify would otherwise pin the
///    in-flight slot for `STALE_IN_FLIGHT_AGE` (5 min) and self-DoS the IP.
/// 4. On success, generate a fresh device token and store its hash.
///
/// The throttle guard is consumed explicitly on every code path that can
/// reach here (`record_success` on the happy path, `record_failure` on every
/// early return). If the request future itself is cancelled — browser
/// disconnects mid-verify, hyper drops the conn — the guard's `Drop` runs
/// fail-closed and records an implicit failure. We intentionally DON'T
/// gate `record_failure` behind a status check; letting Drop handle
/// "everything except the paths I wrote" would make it an `if happy { ... }
/// else { implicit drop }` pattern, which the `consume` flag's placement
/// makes subtly racy (see auth.rs::AttemptGuard docs).
async fn post_auth_login(
    axum::extract::State(state): axum::extract::State<AppState>,
    client_ip: ClientIp,
    axum::Json(body): axum::Json<LoginRequest>,
) -> Response {
    let security = state.security.clone();
    let throttle = security.throttle().clone();
    let db = security.state_db().clone();

    // (1) Reserve in-flight slot or 429.
    let guard = match throttle.try_begin_attempt(client_ip.0).await {
        Ok(g) => g,
        Err(decision) => {
            let retry = decision
                .retry_after
                .unwrap_or(Duration::from_secs(1))
                .as_secs()
                .max(1);
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", retry.to_string())],
                "throttled",
            )
                .into_response();
        }
    };

    if body.password.len() > MAX_PASSWORD_LEN {
        guard.record_failure().await;
        return (axum::http::StatusCode::BAD_REQUEST, "password too long").into_response();
    }

    // (2) Fetch stored password hash. Treat "no password set" as a failed
    // login — leaking the absence lets an attacker distinguish first-run
    // hosts (which admit any POST) from hardened ones.
    let phc = match db.get_web_password_hash().await {
        Ok(Some(h)) => h,
        Ok(None) => {
            guard.record_failure().await;
            return (axum::http::StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to read web password hash");
            guard.record_failure().await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
    };

    // (3) Verify under a hard deadline.
    //
    // Argon2id verify is CPU-bound (~50 ms at our params); run it on the
    // blocking pool so the async runtime keeps responding to other
    // requests. Moving the password into the closure means we don't hold
    // a reference across await points — the spawn_blocking JoinHandle is
    // 'static.
    let password = body.password.clone();
    let phc_owned = phc.clone();
    let verify_handle =
        tokio::task::spawn_blocking(move || auth::verify_password(&password, &phc_owned));
    let verified = match tokio::time::timeout(VERIFY_DEADLINE, verify_handle).await {
        Ok(Ok(v)) => v,
        Ok(Err(join_err)) => {
            tracing::warn!(error = %join_err, "password verify task panicked");
            false
        }
        Err(_elapsed) => {
            tracing::warn!(
                deadline_secs = VERIFY_DEADLINE.as_secs(),
                "password verify exceeded deadline"
            );
            false
        }
    };

    if !verified {
        guard.record_failure().await;
        return (axum::http::StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }

    // (4) Issue a fresh device token. Store only the hash.
    let id = uuid::Uuid::new_v4().to_string();
    let name = body.device_name.clone().unwrap_or_else(|| {
        let short = id.get(..8).unwrap_or("unknown");
        format!("device-{short}")
    });
    let token = auth::generate_device_token();
    if let Err(e) = db.insert_web_device(&id, &name, &token.hash_hex).await {
        tracing::warn!(error = %e, "failed to persist new device token");
        guard.record_failure().await;
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "could not record device",
        )
            .into_response();
    }

    // Best-effort audit row; don't fail the login on audit error.
    let _ = db
        .append_web_audit(
            "web_login_success",
            Some(&id),
            Some(&client_ip.0.to_string()),
            None,
        )
        .await;

    guard.record_success().await;
    axum::Json(LoginResponse {
        device_id: id,
        token: token.raw,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ReauthRequest {
    password: String,
}

/// POST /api/auth/reauth — re-prove the account password, on success
/// refresh this device's sudo-mode freshness in the daemon's reauth
/// registry.
///
/// Gating: the security middleware has already attached an [`AuthedDevice`]
/// by the time we get here, so the caller is a known, non-revoked device.
/// We still require a throttle slot and a password verify on top of that —
/// possession of a device token alone is not enough to clear the sudo gate,
/// which is the whole point.
///
/// Path shape mirrors `post_auth_login` deliberately: same `LoginThrottle`
/// pattern with explicit `record_failure` on every early return (Drop is
/// fail-closed), same [`VERIFY_DEADLINE`] budget, same "treat missing-hash
/// as invalid credentials" info-leak defence. The only extra step is the
/// [`reauth_ipc::signal_mark_device_fresh`] call that tells the daemon to
/// touch the in-memory reauth registry for this device.
async fn post_auth_reauth(
    axum::extract::State(state): axum::extract::State<AppState>,
    client_ip: ClientIp,
    axum::Extension(device): axum::Extension<AuthedDevice>,
    axum::Json(body): axum::Json<ReauthRequest>,
) -> Response {
    let security = state.security.clone();
    let throttle = security.throttle().clone();
    let db = security.state_db().clone();

    let guard = match throttle.try_begin_attempt(client_ip.0).await {
        Ok(g) => g,
        Err(decision) => {
            let retry = decision
                .retry_after
                .unwrap_or(Duration::from_secs(1))
                .as_secs()
                .max(1);
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", retry.to_string())],
                "throttled",
            )
                .into_response();
        }
    };

    if body.password.len() > MAX_PASSWORD_LEN {
        guard.record_failure().await;
        return (axum::http::StatusCode::BAD_REQUEST, "password too long").into_response();
    }

    let phc = match db.get_web_password_hash().await {
        Ok(Some(h)) => h,
        Ok(None) => {
            guard.record_failure().await;
            return (axum::http::StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to read web password hash");
            guard.record_failure().await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
    };

    let password = body.password.clone();
    let phc_owned = phc.clone();
    let verify_handle =
        tokio::task::spawn_blocking(move || auth::verify_password(&password, &phc_owned));
    let verified = match tokio::time::timeout(VERIFY_DEADLINE, verify_handle).await {
        Ok(Ok(v)) => v,
        Ok(Err(join_err)) => {
            tracing::warn!(error = %join_err, "reauth verify task panicked");
            false
        }
        Err(_elapsed) => {
            tracing::warn!(
                deadline_secs = VERIFY_DEADLINE.as_secs(),
                "reauth verify exceeded deadline"
            );
            false
        }
    };

    if !verified {
        guard.record_failure().await;
        return (axum::http::StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }

    // Signal the daemon so its reauth registry learns this device is
    // fresh BEFORE we return 200. Doing the ack-before-response dance
    // prevents a race where the browser's next Approve arrives at the
    // daemon before the touch has landed.
    if let Err(e) = reauth_ipc::signal_mark_device_fresh(&state.socket_path, &device.id).await {
        tracing::warn!(device_id = %device.id, error = %e, "mark_device_fresh IPC failed");
        guard.record_failure().await;
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "daemon not reachable",
        )
            .into_response();
    }

    let _ = db
        .append_web_audit(
            "web_reauth_success",
            Some(&device.id.0),
            Some(&client_ip.0.to_string()),
            None,
        )
        .await;

    guard.record_success().await;
    (axum::http::StatusCode::OK, "ok").into_response()
}

/// GET /api/auth/status — unauthenticated setup-discovery surface.
///
/// Returns `{ password_set, setup_required }`. The frontend hits this
/// before knowing whether to render a login form or a setup page. It's the
/// *only* path that bypasses both the device-token gate AND the
/// setup-required gate; see [`crate::security::path_bypasses_setup_gate`].
///
/// Intentionally leaks whether a password exists. That's the whole point —
/// the caller is about to decide which bootstrap flow to run, and lying
/// here would force us to ship the decision via a cache-buster on the
/// frontend build, which is worse.
async fn get_auth_status(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    let password_set = match state.security.state_db().get_web_password_hash().await {
        Ok(opt) => opt.is_some(),
        Err(e) => {
            tracing::warn!(error = %e, "auth_status: password-hash probe failed");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
    };
    axum::Json(serde_json::json!({
        "password_set": password_set,
        "setup_required": !password_set,
    }))
    .into_response()
}

/// GET /api/auth/profile — origin-aware unauthenticated discovery surface
/// for the active [`AuthProfile`] (itr#310).
///
/// Bearer NOT required (the frontend has to learn the profile before it
/// can render the login screen, and the phone-pair page sees this before
/// it has any token). Gated by the same Origin/Host allowlist as
/// `/api/auth/status` via the security middleware.
///
/// Response shape:
/// ```json
/// {
///   "profile": "local-lan" | "enterprise",
///   "can_enroll_passkey_on_this_origin": bool,
///   "passkey_required": bool,
///   "allow_ephemeral_listener": bool
/// }
/// ```
///
/// `can_enroll_passkey_on_this_origin` is the origin-aware bit — under
/// LocalLAN it returns `false` for RFC1918 IP-literal origins so the SPA
/// can hide the "enroll passkey" button on the phone (WebAuthn forbids
/// IP RP IDs; see `auth_profile::loopback_rp_id_from_origin`).
async fn get_auth_profile(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let policy = &state.auth_policy;
    let can_enroll = origin_can_enroll_passkey(policy, &headers);
    axum::Json(serde_json::json!({
        "profile": policy.profile_str(),
        "can_enroll_passkey_on_this_origin": can_enroll,
        "passkey_required": policy.passkey_required,
        "allow_ephemeral_listener": policy.allow_ephemeral_lan_listener,
    }))
    .into_response()
}

/// Compute `can_enroll_passkey_on_this_origin` from the request's `Origin`
/// header, falling back to `false` if the header is missing or unparsable.
///
/// Falling back to `false` (rather than the policy's "default" RP ID) is
/// intentional: a request without an `Origin` is either a same-origin
/// top-level GET (in which case the SPA already has the policy result
/// from a prior fetch and won't be probing again) or a non-browser caller
/// for whom passkey enrollment isn't a meaningful answer. Either way,
/// "no, you can't enroll here" is the safe, non-leaky default.
fn origin_can_enroll_passkey(policy: &AuthPolicy, headers: &axum::http::HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Ok(origin_url) = webauthn_rs::prelude::Url::parse(origin) else {
        return false;
    };
    policy.rp_id_for_origin(&origin_url).is_some()
}

#[derive(Debug, Deserialize)]
struct SetPasswordRequest {
    password: String,
    /// Matches `LoginRequest::device_name` — so the first device (the one
    /// doing the bootstrap) gets a friendly label on the Devices list
    /// without a follow-up rename.
    #[serde(default)]
    device_name: Option<String>,
}

/// Minimum password length for the onboarding endpoint. The operator sets
/// this via the onboarding UI; enforcing a floor keeps the bar above
/// "obviously bad" without being a full policy engine. The CLI's
/// `wisphive web set-password` accepts anything — that's fine because the
/// CLI requires shell access (a stronger trust signal than "browser reaches
/// the loopback UI").
const MIN_PASSWORD_LEN: usize = 8;

/// Hard ceiling on password length across every auth endpoint
/// (login/reauth/set-password). Argon2's wall-time is insensitive to input
/// length, but the blocking-pool clone + PHC library buffering isn't, and
/// unauth-reachable endpoints must not accept arbitrary-size input. 4 KiB
/// is ~500x longer than any human-typed password and comfortably larger
/// than any plausible passphrase. Shared constant so the three handlers
/// can't drift.
const MAX_PASSWORD_LEN: usize = 4096;

/// Body-size cap for the auth endpoints. The body is a single
/// `{ password, device_name? }` JSON object — 16 KiB is far more than it
/// can legitimately contain and well below axum's 2 MiB default. Applied
/// per-route so long-body endpoints (config PUT) aren't affected.
const AUTH_BODY_LIMIT: usize = 16 * 1024;

/// POST /api/auth/set-password — first-run-only bootstrap over HTTP.
///
/// Used by the onboarding UI (itr#268) so the operator doesn't have to open
/// a terminal and run `wisphive web set-password`. On success the handler
/// both stores the hash AND mints a device token in the same response, so
/// the browser transitions directly from "setup" to "authed" without a
/// separate login round-trip.
///
/// The endpoint is unauthenticated (there's no token to present yet) and
/// exempted from the setup-required gate in
/// [`crate::security::path_bypasses_setup_gate`]. Protection surfaces are:
/// (1) the [`LoginThrottle`] per-IP rate-limit, (2) atomic
/// `try_set_initial_web_password` that returns `false` if a row already
/// exists — serializing the race between two concurrent first-run attempts,
/// (3) Origin/Host allowlist enforced upstream in `security.rs`.
///
/// Once a password exists the endpoint returns 409 permanently; changing or
/// resetting goes through the CLI (`wisphive web reset-password`), which
/// requires shell access on the daemon host.
async fn post_auth_set_password(
    axum::extract::State(state): axum::extract::State<AppState>,
    client_ip: ClientIp,
    axum::Json(body): axum::Json<SetPasswordRequest>,
) -> Response {
    let security = state.security.clone();
    let throttle = security.throttle().clone();
    let db = security.state_db().clone();

    let guard = match throttle.try_begin_attempt(client_ip.0).await {
        Ok(g) => g,
        Err(decision) => {
            let retry = decision
                .retry_after
                .unwrap_or(Duration::from_secs(1))
                .as_secs()
                .max(1);
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", retry.to_string())],
                "throttled",
            )
                .into_response();
        }
    };

    if body.password.len() > MAX_PASSWORD_LEN {
        guard.record_failure().await;
        // Audit even on-size rejections so a scan against the
        // unauthenticated endpoint leaves a forensic trail — 409/400/500
        // are otherwise silent to the operator.
        let _ = db
            .append_web_audit(
                "web_password_set_denied",
                None,
                Some(&client_ip.0.to_string()),
                Some("password_too_long"),
            )
            .await;
        return (axum::http::StatusCode::BAD_REQUEST, "password too long").into_response();
    }

    if body.password.len() < MIN_PASSWORD_LEN {
        guard.record_failure().await;
        let _ = db
            .append_web_audit(
                "web_password_set_denied",
                None,
                Some(&client_ip.0.to_string()),
                Some("password_too_short"),
            )
            .await;
        return (
            axum::http::StatusCode::BAD_REQUEST,
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        )
            .into_response();
    }

    // Hash on the blocking pool — Argon2 is CPU-bound.
    let password = body.password.clone();
    let hash_handle = tokio::task::spawn_blocking(move || auth::hash_password(&password));
    let phc = match tokio::time::timeout(VERIFY_DEADLINE, hash_handle).await {
        Ok(Ok(Ok(phc))) => phc,
        Ok(Ok(Err(e))) => {
            tracing::warn!(error = %e, "argon2 hash failed");
            guard.record_failure().await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
        Ok(Err(join_err)) => {
            tracing::warn!(error = %join_err, "hash task panicked");
            guard.record_failure().await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
        Err(_elapsed) => {
            tracing::warn!(
                deadline_secs = VERIFY_DEADLINE.as_secs(),
                "password hash exceeded deadline"
            );
            guard.record_failure().await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
    };

    match db.try_set_initial_web_password(&phc).await {
        Ok(true) => {} // first-run, row inserted
        Ok(false) => {
            // Password already set — reset flow is CLI-only. Record as a
            // failure for throttle purposes so a script that keeps POSTing
            // can't abuse this as a free probe. Audit the attempt — 409
            // on this endpoint is the "someone scanned an already-
            // provisioned host" signal worth having in forensics.
            guard.record_failure().await;
            let _ = db
                .append_web_audit(
                    "web_password_set_denied",
                    None,
                    Some(&client_ip.0.to_string()),
                    Some("already_set"),
                )
                .await;
            return (
                axum::http::StatusCode::CONFLICT,
                "password already set; use `wisphive web reset-password`",
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, "try_set_initial_web_password failed");
            guard.record_failure().await;
            let _ = db
                .append_web_audit(
                    "web_password_set_denied",
                    None,
                    Some(&client_ip.0.to_string()),
                    Some("db_error"),
                )
                .await;
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
                .into_response();
        }
    }

    // Atomically mint the first device token so the onboarding UI lands
    // in the authenticated shell without a second round-trip. Same shape
    // as LoginResponse so the frontend can reuse the login handler.
    let id = uuid::Uuid::new_v4().to_string();
    let name = body.device_name.clone().unwrap_or_else(|| {
        let short = id.get(..8).unwrap_or("unknown");
        format!("device-{short}")
    });
    let token = auth::generate_device_token();
    if let Err(e) = db.insert_web_device(&id, &name, &token.hash_hex).await {
        tracing::warn!(error = %e, "set-password: failed to persist device token");
        guard.record_failure().await;
        // The password IS set at this point — leave it; the operator can
        // just hit /api/auth/login with the new password to recover.
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "password stored but could not record device; retry login",
        )
            .into_response();
    }

    let _ = db
        .append_web_audit(
            "web_password_set",
            Some(&id),
            Some(&client_ip.0.to_string()),
            None,
        )
        .await;

    guard.record_success().await;
    axum::Json(LoginResponse {
        device_id: id,
        token: token.raw,
    })
    .into_response()
}

/// POST /api/auth/logout — revoke the caller's own device.
///
/// Gated by the security middleware, so reaching this handler means
/// `device` is the verified caller. Revocation is idempotent (`UPDATE …
/// WHERE revoked_at IS NULL`); after we return 200 the next request with
/// the same token fails at `find_web_device_by_token_hash` and comes back
/// as 401.
///
/// We intentionally don't require a password here: logout is a *retracting*
/// action, not a *trust-elevating* one (unlike reauth). The sudo gate is
/// where password re-proof belongs.
async fn post_auth_logout(
    axum::extract::State(state): axum::extract::State<AppState>,
    client_ip: ClientIp,
    axum::Extension(device): axum::Extension<AuthedDevice>,
) -> Response {
    let db = state.security.state_db().clone();
    if let Err(e) = db.revoke_web_device(&device.id.0).await {
        tracing::warn!(error = %e, device_id = %device.id, "logout revoke failed");
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "revoke failed",
        )
            .into_response();
    }
    let _ = db
        .append_web_audit(
            "web_logout",
            Some(&device.id.0),
            Some(&client_ip.0.to_string()),
            None,
        )
        .await;
    (axum::http::StatusCode::OK, "ok").into_response()
}

/// GET /api/me — identify the authenticated device.
///
/// The middleware has already attached an `AuthedDevice`; just echo it
/// back as JSON. Used by the SPA to fill in "logged in as <device_name>"
/// banners without round-tripping through `/api/devices`.
async fn get_me(axum::Extension(device): axum::Extension<AuthedDevice>) -> Response {
    axum::Json(serde_json::json!({
        "device_id": device.id.0,
        "device_name": device.name,
    }))
    .into_response()
}

/// GET /api/devices — list every device ever enrolled, newest first.
///
/// Includes revoked devices so the UI can show history (the `revoked_at`
/// field is how it disambiguates). Any device that has a valid token can
/// see the full list — there's no "admin" tier in the current model; a
/// paired device is a paired device.
async fn get_devices(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    match state.security.state_db().list_web_devices().await {
        Ok(devices) => {
            let json: Vec<serde_json::Value> = devices
                .into_iter()
                .map(|d| {
                    serde_json::json!({
                        "id": d.id,
                        "name": d.name,
                        "created_at": d.created_at,
                        "last_seen_at": d.last_seen_at,
                        "last_ip": d.last_ip,
                        "revoked_at": d.revoked_at,
                    })
                })
                .collect();
            axum::Json(json).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "list_web_devices failed");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "list failed").into_response()
        }
    }
}

/// POST /api/devices/{id}/revoke — revoke any device by id.
///
/// Revoking `device.id.0` (i.e. yourself) is allowed and behaves the same
/// as `/api/auth/logout` from the DB's perspective — the UX case is
/// "I lost my phone, revoke it from my laptop" which means arbitrary-id
/// revocation has to work.
///
/// The audit row records the *actor* device in `device_id` and the
/// *target* device id in `detail` so forensics can reconstruct who
/// revoked whom.
async fn post_revoke_device(
    axum::extract::State(state): axum::extract::State<AppState>,
    client_ip: ClientIp,
    axum::Extension(actor): axum::Extension<AuthedDevice>,
    axum::extract::Path(target_id): axum::extract::Path<String>,
) -> Response {
    let db = state.security.state_db().clone();
    if let Err(e) = db.revoke_web_device(&target_id).await {
        tracing::warn!(error = %e, target = %target_id, "revoke_web_device failed");
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "revoke failed",
        )
            .into_response();
    }
    let _ = db
        .append_web_audit(
            "web_device_revoke",
            Some(&actor.id.0),
            Some(&client_ip.0.to_string()),
            Some(&target_id),
        )
        .await;
    (axum::http::StatusCode::OK, "ok").into_response()
}

fn build_router(state: AppState, dev_mode: bool) -> Router {
    let security = state.security.clone();
    // Body-size cap for the three password-handling endpoints. They're
    // unauth-reachable (login/set-password) or bearer-only (reauth) and
    // the legitimate payload is a small JSON object — a 16 KiB cap
    // narrows the attack surface from axum's 2 MiB default without
    // affecting any real request.
    let auth_body_limit = || axum::extract::DefaultBodyLimit::max(AUTH_BODY_LIMIT);
    let api = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/auth/status", get(get_auth_status))
        .route("/api/auth/profile", get(get_auth_profile))
        .route(
            "/api/auth/login",
            post(post_auth_login).layer(auth_body_limit()),
        )
        .route(
            "/api/auth/set-password",
            post(post_auth_set_password).layer(auth_body_limit()),
        )
        .route("/api/auth/logout", post(post_auth_logout))
        .route(
            "/api/auth/reauth",
            post(post_auth_reauth).layer(auth_body_limit()),
        )
        .route("/api/me", get(get_me))
        .route("/api/devices", get(get_devices))
        .route("/api/devices/{id}/revoke", post(post_revoke_device))
        .route("/api/config", get(get_config).put(put_config));

    let router = if dev_mode {
        api.with_state(state).layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
    } else {
        api.fallback(get(static_handler)).with_state(state)
    };

    router.layer(axum::middleware::from_fn_with_state(
        security,
        security_middleware,
    ))
}

/// Start the web server.
///
/// Production (`dev_mode = false`): serves HTTPS via `axum_server` + the
/// self-signed ECDSA cert from [`crate::tls::ensure_cert`]. The fingerprint
/// and every LAN URL we'd accept are logged at startup so the operator can
/// pin the cert out-of-band.
///
/// Dev (`dev_mode = true`): serves plain HTTP. Vite runs the UI at
/// `http://localhost:5173` and the browser connects to `ws://.../ws`;
/// dragging the user through self-signed-TLS trust in dev on top of the
/// normal Vite workflow isn't worth the pain. The CORS layer in
/// `build_router` already opens things up for that setup.
pub async fn serve(
    socket_path: PathBuf,
    port: u16,
    dev_mode: bool,
    host: [u8; 4],
    auth_profile: AuthProfile,
) -> anyhow::Result<()> {
    // Dev mode serves plain HTTP so Vite (http://localhost:5173) can talk
    // to it without a self-signed-cert trust dance. That's fine on
    // loopback, where the only attacker is one that already owns the
    // host. It is NOT fine on 0.0.0.0 or any LAN address: every password,
    // device token, and decision envelope would travel in the clear over
    // Wi-Fi. Refuse rather than quietly do the wrong thing.
    let bind_host: IpAddr = Ipv4Addr::from(host).into();
    if dev_mode && !bind_host.is_loopback() {
        anyhow::bail!(
            "refusing to serve dev-mode (cleartext HTTP) on non-loopback bind {bind_host}:{port}; \
             drop --web-dev or bind to 127.0.0.1"
        );
    }

    let home_dir = socket_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let config_path = home_dir.join("config.json");
    let db_path = home_dir.join("wisphive.db");

    // Open the same SQLite file the daemon owns; WAL mode + connection
    // pooling means multiple handles in the same process share snapshots
    // safely.
    let state_db = StateDb::open(db_path.to_string_lossy().as_ref()).await?;

    let auth_policy = auth_profile.policy();
    info!(
        profile = auth_policy.profile_str(),
        allow_self_signed = auth_policy.allow_self_signed,
        allow_ephemeral_lan_listener = auth_policy.allow_ephemeral_lan_listener,
        uv_requirement = ?auth_policy.uv_requirement,
        login_throttle_threshold = auth_policy.login_throttle_threshold,
        "auth profile selected"
    );

    // itr#310 stub — scan `web_passkeys.rp_id` for drift vs the active
    // profile. Today this is a no-op (the rp_id column hasn't landed yet;
    // see scan_passkey_rp_id_drift's TODO(itr#311)). Wiring it now means
    // #311 just deletes the early return.
    auth_profile::scan_passkey_rp_id_drift(&state_db, &auth_policy).await;

    let security = SecurityConfig::build(state_db, bind_host, port, dev_mode, auth_policy.clone())?;

    let state = AppState {
        socket_path,
        config_path,
        security,
        auth_policy,
    };

    let app = build_router(state, dev_mode);

    let addr = SocketAddr::from((host, port));
    info!(%addr, dev_mode, "web server starting");

    if dev_mode {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    } else {
        // Production: serve HTTPS with the ECDSA cert we manage under
        // ~/.wisphive/. `ensure_cert` is synchronous and already does its
        // own flock serialization, so concurrent `wisphive daemon start`
        // + `wisphive web` invocations can't clobber each other's keys.
        let cert = tls::ensure_cert(&home_dir, bind_host)?;
        info!(
            fingerprint = %cert.fingerprint_sha256,
            "web TLS cert ready"
        );
        for url in tls::enumerate_lan_urls(bind_host, port) {
            info!(%url, "web listening");
        }
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem(cert.cert_pem, cert.key_pem).await?;
        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod http_tests;
