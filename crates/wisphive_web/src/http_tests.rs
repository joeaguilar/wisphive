//! HTTP-level tests for the security middleware, exercising the real Axum
//! router via `tower::ServiceExt::oneshot`. This lets us build requests with
//! exact Host/Origin/Authorization headers and assert the middleware's
//! response status without standing up a real TCP listener.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tower::ServiceExt;
use wisphive_daemon::state::StateDb;
use wisphive_protocol::{ClientCommand, ClientMessage, PROTOCOL_VERSION, ServerMessage, encode};

use crate::auth::{generate_device_token, hash_password};
use crate::auth_profile::AuthProfile;
use crate::security::{ClientIp, SecurityConfig};
use crate::{AppState, build_router};

const HOST: &str = "127.0.0.1:3100";
const ORIGIN: &str = "http://127.0.0.1:3100";
const PASSWORD: &str = "correct horse battery staple";

/// Open an in-process SQLite DB rooted at a tempdir so each test gets an
/// isolated `web_devices` table. Seeded with a known password so login
/// tests have something to verify against.
async fn test_db() -> (tempfile::TempDir, StateDb) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let phc = hash_password(PASSWORD).unwrap();
    db.set_web_password(&phc).await.unwrap();
    (tmp, db)
}

/// Seed one pre-enrolled device so the tests can present a known-good token.
/// Returns the raw token (base64url) plus the device id.
async fn seed_device(db: &StateDb, name: &str) -> (String, String) {
    let token = generate_device_token();
    let id = uuid::Uuid::new_v4().to_string();
    db.insert_web_device(&id, name, &token.hash_hex)
        .await
        .unwrap();
    (token.raw, id)
}

async fn test_state() -> (tempfile::TempDir, AppState) {
    let (tmp, db) = test_db().await;
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let socket_path = tmp.path().join("wisphive.sock");
    let config_path = tmp.path().join("config.json");
    (
        tmp,
        AppState {
            socket_path,
            config_path,
            security,
            auth_policy: AuthProfile::LocalLAN.policy(),
            passkey_challenges: crate::passkey::ChallengeStore::new(),
        },
    )
}

fn req(method: &str, uri: &str) -> axum::http::request::Builder {
    // Seed a loopback ClientIp so ConnectInfo-free oneshot requests still
    // produce a stable throttle key (otherwise ClientIp falls back to
    // `127.0.0.1`, which is fine but worth being explicit about).
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST);
    builder.extension(ClientIp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))))
}

async fn run_with(state: AppState, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let app = build_router(state, false);
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
}

async fn run(req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let (_tmp, state) = test_state().await;
    run_with(state, req).await
}

#[tokio::test]
async fn api_config_rejects_missing_bearer() {
    let r = req("GET", "/api/config").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_config_rejects_wrong_bearer() {
    let r = req("GET", "/api/config")
        .header("authorization", "Bearer not-a-real-token")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_config_accepts_valid_device_token_via_header() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "iphone").await;
    let r = req("GET", "/api/config")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn api_config_accepts_valid_device_token_via_query() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "laptop").await;
    let r = req("GET", &format!("/api/config?token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn api_config_rejects_revoked_device_token() {
    let (_tmp, state) = test_state().await;
    let (token, id) = seed_device(state.security.state_db(), "old-phone").await;
    state
        .security
        .state_db()
        .revoke_web_device(&id)
        .await
        .unwrap();
    let r = req("GET", "/api/config")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn evil_origin_is_rejected_on_api() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "x").await;
    let r = req("GET", "/api/config")
        .header("origin", "http://evil.com")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bad_host_is_rejected_on_api() {
    let r = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("host", "evil.example")
        .header("authorization", "Bearer anything")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_host_is_rejected() {
    let r = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("authorization", "Bearer anything")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// The old `/api/web-token` route is retired. The router no longer
/// registers it, and the fallback is the SPA static handler which 404s on
/// a missing asset.
#[tokio::test]
async fn web_token_bootstrap_returns_404() {
    let r = req("GET", "/api/web-token").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ws_without_token_returns_401() {
    let r = req("GET", "/ws")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ws_with_evil_origin_returns_403() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "x").await;
    let r = req("GET", &format!("/ws?token={token}"))
        .header("origin", "http://evil.com")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ws_with_valid_token_and_origin_passes_middleware() {
    // `oneshot` doesn't carry an `OnUpgrade` extension (only a real hyper
    // server does), so the ws handler itself returns a non-upgrade error.
    // What matters for this test is that the middleware let the request
    // through to the handler — i.e. we did *not* get 401/403.
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "y").await;
    let r = req("GET", &format!("/ws?token={token}"))
        .header("origin", ORIGIN)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    assert_ne!(status, StatusCode::FORBIDDEN);
}

// ── /api/auth/login ────────────────────────────────────────────────────

#[tokio::test]
async fn login_with_correct_password_issues_device_token() {
    let (_tmp, state) = test_state().await;
    let body = serde_json::json!({ "password": PASSWORD, "device_name": "iphone" });
    let r = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = parsed["token"].as_str().unwrap().to_string();
    let device_id = parsed["device_id"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    assert!(!device_id.is_empty());

    // The returned token must work on a follow-up protected request.
    let r2 = req("GET", "/api/config")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (s2, _) = run_with(state, r2).await;
    assert_eq!(s2, StatusCode::OK);
}

#[tokio::test]
async fn login_with_wrong_password_returns_401_and_bumps_throttle() {
    let (_tmp, state) = test_state().await;
    let body = serde_json::json!({ "password": "wrong-password" });
    let r = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // peek is the read-only UI hint — after one failed attempt the IP
    // should be in a short (250 ms) lockout window.
    let loopback: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
    let retry = state.security.throttle().peek(loopback).await;
    assert!(
        retry.is_some(),
        "wrong-password attempt should have bumped the throttle"
    );
}

/// Pre-itr#214 contract: login-when-no-password returned 401 to avoid
/// leaking "setup-required" state via the credential endpoint. Post-
/// itr#214 that's superseded by the setup-required gate: the SPA is
/// *supposed* to learn setup state from `/api/auth/status` and 503 from
/// the gate is the machine-readable signal that login shouldn't even be
/// attempted yet. So the contract on this path is now:
///
///   - Setup-required mode: 503 `setup_required` (the gate's response),
///     NOT the handler's 401. The frontend has to distinguish "no
///     account" from "wrong password" and the gate is how it does so.
///   - The handler's body is still non-leaky — it's never reached, so
///     `no password` / `not set` cannot appear in the wire bytes.
#[tokio::test]
async fn login_when_no_password_set_returns_503_setup_required() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };

    let body = serde_json::json!({ "password": "anything" });
    let r = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // The body is JSON with a stable `error: setup_required` discriminant;
    // the old "no password" / "not set" substring test doesn't apply to
    // the gate's response, which never narrates the DB state in prose.
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"], "setup_required");
}

// ── /api/auth/reauth ───────────────────────────────────────────────────
//
// Reauth needs the security middleware to have attached an `AuthedDevice`,
// so tests present a valid device token on every request. The "does it
// actually mark the device fresh?" acceptance is covered end-to-end by the
// daemon integration tests (server_integration.rs); here we cover:
//
// - Missing token → 401 (middleware short-circuits before reauth runs).
// - Wrong password → 401 + throttle bump; no daemon traffic needed.
// - Daemon unreachable → 503; we surface the IPC failure without 200-ing.
// - Correct password + a fake daemon that acks → 200, audit row recorded.

/// Spin up a toy UnixListener at `socket_path` that pretends to be the
/// daemon for exactly one `MarkDeviceFresh` exchange. Sends a Welcome +
/// empty AgentsSnapshot + empty QueueSnapshot (mirroring `handle_tui`'s
/// prelude), then reads the next envelope, and, if it's MarkDeviceFresh
/// with a device_id, replies with `MarkDeviceFreshAck`.
///
/// Returns the join handle so the test can await it and assert the
/// observed device id.
fn spawn_fake_daemon(socket_path: PathBuf) -> tokio::task::JoinHandle<Option<String>> {
    tokio::spawn(async move {
        let listener = UnixListener::bind(&socket_path).expect("bind fake daemon socket");
        let (stream, _) = listener.accept().await.expect("accept fake daemon conn");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        // Hello
        let _ = lines.next_line().await.expect("read hello");
        let welcome = encode(&ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
        })
        .unwrap();
        writer.write_all(welcome.as_bytes()).await.unwrap();
        // handle_tui sends AgentsSnapshot + QueueSnapshot before looping; mirror
        // that so the client-side ack-waiter has the shape it expects.
        let agents = encode(&ServerMessage::AgentsSnapshot { agents: vec![] }).unwrap();
        writer.write_all(agents.as_bytes()).await.unwrap();
        let snap = encode(&ServerMessage::QueueSnapshot { items: vec![] }).unwrap();
        writer.write_all(snap.as_bytes()).await.unwrap();

        // Expect MarkDeviceFresh next.
        let Ok(Some(text)) = lines.next_line().await else {
            return None;
        };
        let cmd: ClientCommand = wisphive_protocol::decode(&text).ok()?;
        match cmd.body {
            ClientMessage::MarkDeviceFresh => {}
            _ => return None,
        }
        let device_id = cmd.device_id.map(|d| d.0)?;
        let ack = encode(&ServerMessage::MarkDeviceFreshAck {
            device_id: device_id.clone(),
        })
        .unwrap();
        writer.write_all(ack.as_bytes()).await.unwrap();
        Some(device_id)
    })
}

#[tokio::test]
async fn reauth_without_device_token_returns_401() {
    let r = req("POST", "/api/auth/reauth")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reauth_with_wrong_password_returns_401_and_bumps_throttle() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "iphone").await;

    let r = req("POST", "/api/auth/reauth")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({ "password": "wrong" }).to_string(),
        ))
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let loopback: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
    assert!(
        state.security.throttle().peek(loopback).await.is_some(),
        "wrong-password reauth must bump the throttle"
    );
}

#[tokio::test]
async fn reauth_succeeds_when_daemon_acks_mark_device_fresh() {
    let (tmp, state) = test_state().await;
    let (token, device_id) = seed_device(state.security.state_db(), "iphone").await;

    // Stand up a fake daemon at the state's socket_path so reauth_ipc has
    // something to talk to.
    let fake = spawn_fake_daemon(state.socket_path.clone());
    // Give the listener time to actually bind before the HTTP handler
    // tries to connect.
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    let r = req("POST", "/api/auth/reauth")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({ "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let (status, _body) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);

    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), fake)
        .await
        .expect("fake daemon did not finish")
        .expect("fake daemon task panicked")
        .expect("fake daemon did not record a device id");
    assert_eq!(observed, device_id);
    // tmp must outlive the socket path until we've confirmed the ack.
    drop(tmp);
}

#[tokio::test]
async fn reauth_returns_503_when_daemon_socket_unreachable() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "iphone").await;
    // Do not spawn a fake daemon — state.socket_path does not exist.

    let r = req("POST", "/api/auth/reauth")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({ "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// A second back-to-back login attempt from the same IP should hit the
/// post-failure lockout (we just bumped it on attempt #1) and come back
/// as `429 Too Many Requests` with a `Retry-After` header, not `401`.
#[tokio::test]
async fn second_attempt_during_lockout_returns_429_with_retry_after() {
    let (_tmp, state) = test_state().await;

    // Attempt #1: wrong password → 401 + throttle bump.
    let r1 = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "password": "nope" }).to_string(),
        ))
        .unwrap();
    let (s1, _) = run_with(state.clone(), r1).await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);

    // Attempt #2 immediately after: should be rejected by the lockout.
    // Even correct credentials lose — the throttle doesn't know the
    // difference, by design (the whole point is to slow brute force).
    let r2 = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let app = build_router(state, false);
    let res = app.oneshot(r2).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry = res
        .headers()
        .get("retry-after")
        .expect("429 must include Retry-After")
        .to_str()
        .unwrap()
        .to_string();
    let n: u64 = retry.parse().expect("Retry-After should be an integer");
    assert!(n >= 1, "Retry-After should be at least 1 second, got {n}");
}

// ── /api/auth/status ───────────────────────────────────────────────────

/// The SPA hits /api/auth/status before it knows whether to render a login
/// form, so it MUST be reachable without a device token — and the response
/// MUST tell the truth about whether a password is set.
#[tokio::test]
async fn auth_status_reports_password_set_true_when_seeded() {
    // test_state seeds the password in test_db.
    let (_tmp, state) = test_state().await;
    let r = req("GET", "/api/auth/status").body(Body::empty()).unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["password_set"], serde_json::Value::Bool(true));
    assert_eq!(parsed["setup_required"], serde_json::Value::Bool(false));
}

#[tokio::test]
async fn auth_status_reports_setup_required_on_fresh_host() {
    // Hand-built state with no password set — exercises the setup-required
    // branch without going through the login handler.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = req("GET", "/api/auth/status").body(Body::empty()).unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["password_set"], serde_json::Value::Bool(false));
    assert_eq!(parsed["setup_required"], serde_json::Value::Bool(true));
}

#[tokio::test]
async fn auth_status_requires_no_bearer() {
    // Same assertion as above but explicit: /api/auth/status is the one
    // path that MUST NOT 401 on missing token, or the setup bootstrap
    // fundamentally breaks.
    let r = req("GET", "/api/auth/status").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::OK);
}

// ── setup-required gate ────────────────────────────────────────────────

/// Fresh host with no web password: every /api/* route except
/// /api/auth/status must 503. This is the "don't let the UI leak into a
/// login flow before setup" rail.
#[tokio::test]
async fn setup_required_blocks_protected_api_routes_with_503() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };

    // Token-gated endpoint — the setup gate must fire *before* the token
    // check, or the operator sees 401 and assumes a credential problem
    // rather than an uninitialized host.
    let r = req("GET", "/api/config").body(Body::empty()).unwrap();
    let (status, body) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"], "setup_required");

    // /api/auth/login should also 503 in setup-required mode: there IS no
    // password, so returning the handler's usual 401 would be misleading
    // ("wrong credentials" vs "no credentials exist yet"). The SPA is
    // expected to route to /setup based on /api/auth/status first.
    let r = req("POST", "/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "password": "x" }).to_string(),
        ))
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// Static content and `/api/auth/status` must still be reachable in
/// setup-required mode — otherwise the browser hitting `/` hangs before
/// it has anything to bootstrap from.
#[tokio::test]
async fn setup_required_lets_auth_status_through() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = req("GET", "/api/auth/status").body(Body::empty()).unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
}

// ── /api/auth/profile (itr#310) ────────────────────────────────────────

/// `/api/auth/profile` is the origin-aware discovery surface. Under
/// LocalLAN with a loopback `Origin`, the SPA should learn it CAN offer
/// passkey enrollment on this origin.
#[tokio::test]
async fn auth_profile_local_lan_loopback_can_enroll() {
    let (_tmp, state) = test_state().await;
    let r = req("GET", "/api/auth/profile")
        .header("origin", ORIGIN) // http://127.0.0.1:3100
        .body(Body::empty())
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["profile"], "local-lan");
    assert_eq!(v["can_enroll_passkey_on_this_origin"], true);
    // v1 keeps both profiles at passkey_required=false (password login is
    // always permitted).
    assert_eq!(v["passkey_required"], false);
    // LocalLAN's ephemeral LAN listener is enabled — phone-pair UI hangs
    // off this bit.
    assert_eq!(v["allow_ephemeral_listener"], true);
}

/// The load-bearing case from the spec: a phone reaching the daemon at a
/// LAN-IP origin must get `can_enroll_passkey_on_this_origin: false` so
/// the SPA hides the "enroll passkey" button. WebAuthn forbids IP-literal
/// RP IDs, so silently offering the button would let the user mash it and
/// get an opaque browser error.
#[tokio::test]
async fn auth_profile_local_lan_lan_ip_origin_cannot_enroll() {
    // test_state's allowlist only contains the loopback ORIGIN, so we have
    // to build a custom state whose allowlist accepts the LAN-IP origin
    // (otherwise the security middleware 403s before we reach the handler).
    let (tmp, db) = test_db().await;
    let lan_origin = "https://192.168.1.42:8443".to_string();
    let security = SecurityConfig::for_test(
        vec![lan_origin.clone()],
        vec!["192.168.1.42:8443".to_string()],
        db.clone(),
    );
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = Request::builder()
        .method("GET")
        .uri("/api/auth/profile")
        .header("host", "192.168.1.42:8443")
        .header("origin", &lan_origin)
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42))))
        .body(Body::empty())
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["profile"], "local-lan");
    assert_eq!(v["can_enroll_passkey_on_this_origin"], false);
}

/// Bearer NOT required for `/api/auth/profile` — same bootstrap-discovery
/// guarantee as `/api/auth/status`.
#[tokio::test]
async fn auth_profile_requires_no_bearer() {
    let r = req("GET", "/api/auth/profile")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::OK);
}

/// `/api/auth/profile` must also bypass the setup-required gate — the
/// onboarding screen (no password yet) reads it to decide whether the
/// "enroll passkey" affordance is even meaningful.
#[tokio::test]
async fn auth_profile_reachable_in_setup_required_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = req("GET", "/api/auth/profile")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
}

/// Enterprise profile: any request origin (that passes the Origin/Host
/// allowlist) gets the static RP ID, so `can_enroll_passkey_on_this_origin`
/// is always `true`. UV-required and login-throttle-3 also surface through
/// the public JSON via the related flags.
#[tokio::test]
async fn auth_profile_enterprise_always_can_enroll() {
    let (tmp, db) = test_db().await;
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let policy = AuthProfile::Enterprise {
        rp_id: crate::auth_profile::RpId("wisphive.example.com".to_string()),
        rp_origin: webauthn_rs::prelude::Url::parse("https://wisphive.example.com").unwrap(),
    }
    .policy();
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: policy,
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = req("GET", "/api/auth/profile")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["profile"], "enterprise");
    assert_eq!(v["can_enroll_passkey_on_this_origin"], true);
    // Enterprise disables the ephemeral LAN listener (#271/#272 dormant).
    assert_eq!(v["allow_ephemeral_listener"], false);
}

// ── /api/auth/logout ───────────────────────────────────────────────────

#[tokio::test]
async fn logout_revokes_device_and_next_request_401s() {
    let (_tmp, state) = test_state().await;
    let (token, device_id) = seed_device(state.security.state_db(), "iphone").await;

    // Logout with the token — expect 200, audit row, and the device row
    // flipped to revoked.
    let r = req("POST", "/api/auth/logout")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);

    // Confirm the DB row is revoked.
    let devices = state.security.state_db().list_web_devices().await.unwrap();
    let found = devices.iter().find(|d| d.id == device_id).unwrap();
    assert!(
        found.revoked_at.is_some(),
        "logout must flip revoked_at on the device row"
    );

    // A follow-up request with the same token must come back as 401 —
    // the middleware's lookup filters revoked rows, so a revoked token is
    // indistinguishable from an unknown one (same 401 response).
    let r2 = req("GET", "/api/config")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (s2, _) = run_with(state, r2).await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_without_token_returns_401() {
    let r = req("POST", "/api/auth/logout").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── /api/me ────────────────────────────────────────────────────────────

#[tokio::test]
async fn me_returns_authenticated_device_identity() {
    let (_tmp, state) = test_state().await;
    let (token, device_id) = seed_device(state.security.state_db(), "laptop-work").await;

    let r = req("GET", "/api/me")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["device_id"].as_str().unwrap(), device_id);
    assert_eq!(parsed["device_name"].as_str().unwrap(), "laptop-work");
}

#[tokio::test]
async fn me_without_token_returns_401() {
    let r = req("GET", "/api/me").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── /api/devices + /api/devices/{id}/revoke ────────────────────────────

#[tokio::test]
async fn devices_lists_both_active_and_revoked() {
    let (_tmp, state) = test_state().await;
    let (token, _) = seed_device(state.security.state_db(), "iphone").await;
    let (_, other_id) = seed_device(state.security.state_db(), "old-laptop").await;
    state
        .security
        .state_db()
        .revoke_web_device(&other_id)
        .await
        .unwrap();

    let r = req("GET", "/api/devices")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed.len(), 2, "list should include revoked devices");
    let revoked = parsed
        .iter()
        .find(|d| d["id"].as_str() == Some(other_id.as_str()))
        .unwrap();
    assert!(
        !revoked["revoked_at"].is_null(),
        "revoked device must report revoked_at in the JSON"
    );
}

#[tokio::test]
async fn devices_revoke_other_device() {
    let (_tmp, state) = test_state().await;
    let (actor_token, _) = seed_device(state.security.state_db(), "laptop").await;
    let (victim_token, victim_id) = seed_device(state.security.state_db(), "phone").await;

    // Actor revokes the victim.
    let r = req("POST", &format!("/api/devices/{victim_id}/revoke"))
        .header("authorization", format!("Bearer {actor_token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);

    // Victim's token should now be dead.
    let r2 = req("GET", "/api/config")
        .header("authorization", format!("Bearer {victim_token}"))
        .body(Body::empty())
        .unwrap();
    let (s2, _) = run_with(state.clone(), r2).await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);

    // Actor's token should still work — revocation is per-device, not
    // per-account.
    let r3 = req("GET", "/api/config")
        .header("authorization", format!("Bearer {actor_token}"))
        .body(Body::empty())
        .unwrap();
    let (s3, _) = run_with(state, r3).await;
    assert_eq!(s3, StatusCode::OK);
}

/// `/api/devices/{self.id}/revoke` is the symmetric partner to
/// `/api/auth/logout`: same effect (the caller's token gets revoked,
/// next request 401s), different UX surface (the devices-list "remove
/// this device" row, vs the logout button). The handler's docstring
/// claims this works; pin it so a future "don't let a device revoke
/// itself" overzealous-refactor regresses noisily instead of silently.
#[tokio::test]
async fn devices_revoke_self_ends_session() {
    let (_tmp, state) = test_state().await;
    let (token, device_id) = seed_device(state.security.state_db(), "mine").await;

    let r = req("POST", &format!("/api/devices/{device_id}/revoke"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);

    // Next request with the now-revoked token must 401 — the handler
    // succeeded, even though the caller revoked their own credential.
    let r2 = req("GET", "/api/config")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (s2, _) = run_with(state, r2).await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn devices_revoke_is_idempotent() {
    let (_tmp, state) = test_state().await;
    let (actor_token, _) = seed_device(state.security.state_db(), "laptop").await;
    let (_, victim_id) = seed_device(state.security.state_db(), "phone").await;

    for _ in 0..2 {
        let r = req("POST", &format!("/api/devices/{victim_id}/revoke"))
            .header("authorization", format!("Bearer {actor_token}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = run_with(state.clone(), r).await;
        assert_eq!(status, StatusCode::OK, "idempotent revoke must stay 200");
    }
}

#[tokio::test]
async fn devices_revoke_without_token_returns_401() {
    let r = req("POST", "/api/devices/some-id/revoke")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn devices_list_without_token_returns_401() {
    let r = req("GET", "/api/devices").body(Body::empty()).unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── /api/auth/set-password (itr#268) ────────────────────────────────

/// Variant of test_state that leaves web_password empty — needed for
/// first-run bootstrap tests. The shared helper pre-seeds a password so
/// login tests have something to verify against.
async fn test_state_no_password() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("wisphive.db");
    let db = StateDb::open(db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    let security =
        SecurityConfig::for_test(vec![ORIGIN.to_string()], vec![HOST.to_string()], db.clone());
    let socket_path = tmp.path().join("wisphive.sock");
    let config_path = tmp.path().join("config.json");
    (
        tmp,
        AppState {
            socket_path,
            config_path,
            security,
            auth_policy: AuthProfile::LocalLAN.policy(),
            passkey_challenges: crate::passkey::ChallengeStore::new(),
        },
    )
}

#[tokio::test]
async fn set_password_first_run_issues_device_token() {
    let (_tmp, state) = test_state_no_password().await;
    let body = r#"{"password":"hunter2-onboard","device_name":"laptop"}"#;
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, body) = run_with(state.clone(), r).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first-run set-password should succeed"
    );
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("device_id").and_then(|s| s.as_str()).is_some());
    assert!(v.get("token").and_then(|s| s.as_str()).is_some());
    // Password is now persisted — subsequent set attempts must fail.
    assert!(
        state
            .security
            .state_db()
            .get_web_password_hash()
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn set_password_rejects_when_already_set() {
    // Shared test_state() already seeds a password.
    let (_tmp, state) = test_state().await;
    let body = r#"{"password":"late-to-the-party"}"#;
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "second set-password must 409 to prevent takeover"
    );
}

#[tokio::test]
async fn set_password_rejects_weak_password() {
    let (_tmp, state) = test_state_no_password().await;
    let body = r#"{"password":"short"}"#;
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Must NOT persist the weak password.
    assert!(
        state
            .security
            .state_db()
            .get_web_password_hash()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn set_password_rejects_oversized_password() {
    // Defense against DoS via Argon2 on arbitrarily-long input. The cap
    // is 4096 characters; 10k comfortably exceeds it.
    let (_tmp, state) = test_state_no_password().await;
    let big = "x".repeat(10_000);
    let body = serde_json::json!({ "password": big }).to_string();
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Must not persist.
    assert!(
        state
            .security
            .state_db()
            .get_web_password_hash()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn set_password_rejects_body_over_limit() {
    // Body-size cap on the route (axum DefaultBodyLimit). A 32 KiB body
    // — well under axum's 2 MiB default but well over our 16 KiB cap —
    // should be rejected before the handler sees it.
    let (_tmp, state) = test_state_no_password().await;
    let huge = "x".repeat(32 * 1024);
    let body = serde_json::json!({ "password": huge }).to_string();
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "oversized body must 413 before reaching the handler"
    );
}

#[tokio::test]
async fn set_password_bypasses_setup_required_gate() {
    // Regression for the security.rs path_bypasses_setup_gate wiring —
    // without the bypass, this would 503 under the setup-required gate
    // instead of reaching the handler.
    let (_tmp, state) = test_state_no_password().await;
    let body = r#"{"password":"hunter2-onboard"}"#;
    let r = req("POST", "/api/auth/set-password")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_ne!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "setup gate must bypass /api/auth/set-password"
    );
    assert_eq!(status, StatusCode::OK);
}

// ── itr#311: WebAuthn passkey HTTP-level tests ─────────────────────────
//
// Full register/login round-trips require a SoftPasskey test authenticator,
// which webauthn-rs 0.5 does not ship publicly (the `webauthn-authenticator-rs`
// crate covers that, but it's not part of our dep graph). These tests cover
// every code path we OWN that doesn't need a real cryptographic round-trip:
//
//   - Register on a LAN-IP origin under LocalLAN → 400 with the
//     `passkey_unavailable_on_this_origin` discriminant.
//   - Register/start on a loopback origin → 200, returns a session_id +
//     PublicKey creation options.
//   - Register/finish with an unknown session_id → 400 (single-use
//     enforced by ChallengeStore.take()).
//   - Register/finish replay → 400 (second `take` returns None even
//     against a valid session_id).
//   - Login/start unauthenticated on a loopback origin → 200.
//   - Login/finish with an unknown credential → 401 + audit failure.
//   - Login throttle: 6 failed login finishes from the same IP push the
//     IP past `login_throttle_threshold`.
//   - Enterprise profile + sudo-required policy → register/start 403 with
//     `sudo_required_for_passkey_register` discriminant (placeholder until
//     itr#313 wires the freshness probe).
//   - LAN-IP login start also surfaces `passkey_unavailable_on_this_origin`.
//
// All routes here go through the production router so the security
// middleware + auth_policy threading + body limits all fire.

#[tokio::test]
async fn passkey_register_start_on_loopback_origin_returns_session() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "laptop").await;
    let body = serde_json::json!({});
    let r = req("POST", "/api/auth/passkey/register/start")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "register/start should accept loopback origin"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        parsed["session_id"].is_string(),
        "session_id must be present"
    );
    // Response is the flattened WebAuthn `CreationChallengeResponse`
    // with `session_id` injected, so the SPA can pass `body` straight
    // into `navigator.credentials.create({ publicKey: body.publicKey })`.
    assert!(parsed["publicKey"]["challenge"].is_string());
    assert_eq!(parsed["publicKey"]["rp"]["id"], "localhost");
}

#[tokio::test]
async fn passkey_register_start_on_lan_ip_origin_returns_400() {
    let (_tmp, state) = test_state().await;
    let (_token, _id) = seed_device(state.security.state_db(), "phone").await;
    // Override the request's Origin to an RFC1918 IP literal that
    // `AuthPolicy::rp_id_for_origin` resolves to None. The Host header
    // (which the security middleware allowlists separately) stays at
    // 127.0.0.1:3100 because the test allowlist only seeds that.
    //
    // Reach into security to add the LAN origin to the allowlist; without
    // it the security middleware rejects with 403 before our handler runs.
    // The for_test SecurityConfig doesn't expose mutation, so we
    // construct a tailored state here.
    let lan_origin = "https://192.168.1.42:8443";
    let (tmp2, db2) = test_db().await;
    let security = SecurityConfig::for_test(
        vec![lan_origin.to_string(), ORIGIN.to_string()],
        vec![HOST.to_string()],
        db2.clone(),
    );
    let (token, _id) = seed_device(&db2, "lan-phone").await;
    let state2 = AppState {
        socket_path: tmp2.path().join("wisphive.sock"),
        config_path: tmp2.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    drop(state);
    let body = serde_json::json!({});
    let r = req("POST", "/api/auth/passkey/register/start")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", lan_origin)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state2, r).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "LAN-IP origin under LocalLAN must be a hard 400, not a silent fallback"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"], "passkey_unavailable_on_this_origin");
}

#[tokio::test]
async fn passkey_register_finish_with_unknown_session_returns_400() {
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "laptop").await;
    // A syntactically-valid RegisterPublicKeyCredential takes a fair
    // amount of fixture — we don't need a valid one here because the
    // session-id lookup short-circuits before the credential is parsed
    // by webauthn-rs. Send a minimal shape that satisfies serde.
    let body = serde_json::json!({
        "session_id": "this-session-was-never-issued",
        "credential": {
            "id": "ZmFrZQ",
            "rawId": "ZmFrZQ",
            "type": "public-key",
            "extensions": {},
            "response": {
                "attestationObject": "ZmFrZQ",
                "clientDataJSON": "ZmFrZQ"
            }
        }
    });
    let r = req("POST", "/api/auth/passkey/register/finish")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("unknown or expired session"),
        "unexpected body: {text}"
    );
}

#[tokio::test]
async fn passkey_register_finish_replay_after_take_returns_400() {
    // Acquire a real session_id via register/start, then submit
    // register/finish twice. The second attempt MUST 400 because
    // `ChallengeStore::take` removed the entry on the first call.
    let (_tmp, state) = test_state().await;
    let (token, _id) = seed_device(state.security.state_db(), "laptop").await;
    let body = serde_json::json!({});
    let r = req("POST", "/api/auth/passkey/register/start")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state.clone(), r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = parsed["session_id"].as_str().unwrap().to_string();

    // First finish: invalid credential payload, but the take() consumes
    // the entry. We expect 400 (webauthn finish fails) and that's fine.
    let finish_body = serde_json::json!({
        "session_id": session_id,
        "credential": {
            "id": "ZmFrZQ",
            "rawId": "ZmFrZQ",
            "type": "public-key",
            "extensions": {},
            "response": {
                "attestationObject": "ZmFrZQ",
                "clientDataJSON": "ZmFrZQ"
            }
        }
    });
    let r1 = req("POST", "/api/auth/passkey/register/finish")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(finish_body.to_string()))
        .unwrap();
    let (status1, _) = run_with(state.clone(), r1).await;
    // Could be 400 (credential parse fail) — either way the session is now
    // consumed.
    assert!(
        status1 == StatusCode::BAD_REQUEST || status1 == StatusCode::OK,
        "first finish returned {status1}"
    );

    // Second finish: must 400 with "unknown or expired session".
    let r2 = req("POST", "/api/auth/passkey/register/finish")
        .header("authorization", format!("Bearer {token}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(finish_body.to_string()))
        .unwrap();
    let (status2, body2) = run_with(state, r2).await;
    assert_eq!(
        status2,
        StatusCode::BAD_REQUEST,
        "replay against the same session_id must 400"
    );
    let text = String::from_utf8_lossy(&body2);
    assert!(text.contains("unknown or expired session"), "body: {text}");
}

#[tokio::test]
async fn passkey_login_start_on_loopback_is_unauthenticated() {
    let (_tmp, state) = test_state().await;
    // Note: NO bearer token here. /api/auth/passkey/login/start MUST be
    // reachable without one — it's the bootstrap for an unauth caller,
    // gated only by the throttle + Origin/Host allowlist.
    let body = serde_json::json!({});
    let r = req("POST", "/api/auth/passkey/login/start")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed["session_id"].is_string());
    assert!(parsed["publicKey"]["challenge"].is_string());
}

#[tokio::test]
async fn passkey_login_start_on_lan_ip_returns_400() {
    // Same setup as register: LAN-IP origin must surface
    // `passkey_unavailable_on_this_origin`.
    let lan_origin = "https://10.0.0.5:8443";
    let (tmp, db) = test_db().await;
    let security = SecurityConfig::for_test(
        vec![lan_origin.to_string(), ORIGIN.to_string()],
        vec![HOST.to_string()],
        db.clone(),
    );
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: AuthProfile::LocalLAN.policy(),
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let body = serde_json::json!({});
    let r = req("POST", "/api/auth/passkey/login/start")
        .header("origin", lan_origin)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"], "passkey_unavailable_on_this_origin");
}

#[tokio::test]
async fn passkey_login_finish_with_unknown_credential_returns_401_and_audits() {
    let (_tmp, state) = test_state().await;

    // Acquire a session via login/start so we have a real session_id
    // (otherwise we'd 400 on session lookup before the credential
    // dispatch). Send an unrecognized credential id to /finish.
    let start_r = req("POST", "/api/auth/passkey/login/start")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({}).to_string()))
        .unwrap();
    let (s, body) = run_with(state.clone(), start_r).await;
    assert_eq!(s, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = parsed["session_id"].as_str().unwrap().to_string();

    let body = serde_json::json!({
        "session_id": session_id,
        "credential": {
            "id": "dW5rbm93bg",
            "rawId": "dW5rbm93bg",
            "type": "public-key",
            "extensions": {},
            "response": {
                "authenticatorData": "ZmFrZQ",
                "clientDataJSON": "ZmFrZQ",
                "signature": "ZmFrZQ",
                "userHandle": null
            }
        }
    });
    let r = req("POST", "/api/auth/passkey/login/finish")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let db_for_assertion = state.security.state_db().clone();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Audit row recorded with discriminant `unknown_credential`.
    let audit = db_for_assertion.list_web_audit(100).await.unwrap();
    let found = audit.iter().any(|row| {
        row.event == "passkey_login_failure" && row.detail.as_deref() == Some("unknown_credential")
    });
    assert!(
        found,
        "expected passkey_login_failure / unknown_credential audit row"
    );
}

#[tokio::test]
async fn passkey_register_under_enterprise_with_sudo_required_returns_403() {
    // Enterprise profile turns require_sudo_for_passkey_register on; until
    // itr#313 lands the freshness probe, the handler short-circuits with
    // 403 + sudo_required_for_passkey_register discriminant. Lock that
    // behaviour in.
    let (tmp, db) = test_db().await;
    let rp_origin = "https://wisphive.example.com";
    let policy = AuthProfile::Enterprise {
        rp_id: crate::auth_profile::RpId("wisphive.example.com".to_string()),
        rp_origin: webauthn_rs::prelude::Url::parse(rp_origin).unwrap(),
    }
    .policy();
    let security = SecurityConfig::for_test_with_policy(
        vec![rp_origin.to_string()],
        vec!["wisphive.example.com".to_string()],
        db.clone(),
        policy.clone(),
    );
    let (token, _id) = seed_device(&db, "laptop").await;
    let state = AppState {
        socket_path: tmp.path().join("wisphive.sock"),
        config_path: tmp.path().join("config.json"),
        security,
        auth_policy: policy,
        passkey_challenges: crate::passkey::ChallengeStore::new(),
    };
    let r = Request::builder()
        .method("POST")
        .uri("/api/auth/passkey/register/start")
        .header("host", "wisphive.example.com")
        .header("origin", rp_origin)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))))
        .body(Body::from(serde_json::json!({}).to_string()))
        .unwrap();
    let (status, body) = run_with(state, r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"], "sudo_required_for_passkey_register");
}

#[tokio::test]
async fn passkey_login_finish_bumps_login_throttle_on_failure() {
    // Two failed passkey login finishes from the same IP should leave
    // the IP in cooldown — the second call returns 401 once we exhaust
    // the throttle slot. Combined with password fails this would close
    // off the IP entirely; here we just assert one passkey fail
    // produces a non-empty throttle peek.
    let (_tmp, state) = test_state().await;
    let throttle = state.security.throttle().clone();

    // Acquire a session id, then fail finish with an unknown credential.
    let start_r = req("POST", "/api/auth/passkey/login/start")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let (_, body) = run_with(state.clone(), start_r).await;
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = parsed["session_id"].as_str().unwrap().to_string();

    let fail_body = serde_json::json!({
        "session_id": session_id,
        "credential": {
            "id": "dW5rbm93bg",
            "rawId": "dW5rbm93bg",
            "type": "public-key",
            "extensions": {},
            "response": {
                "authenticatorData": "ZmFrZQ",
                "clientDataJSON": "ZmFrZQ",
                "signature": "ZmFrZQ",
                "userHandle": null
            }
        }
    });
    let r = req("POST", "/api/auth/passkey/login/finish")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(fail_body.to_string()))
        .unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Throttle should have an entry for the loopback IP now (the test
    // harness seeds ClientIp(127.0.0.1)).
    let peek = throttle.peek(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))).await;
    assert!(
        peek.is_some(),
        "failed passkey login must register an IP in the throttle"
    );
}

#[tokio::test]
async fn passkey_login_start_does_not_wipe_throttle_after_failures() {
    // M1 regression: previously `/login/start` called `record_success`
    // which wipes the per-IP failure counter. An attacker could climb
    // toward lockout via `/login/finish` failures, then call `/start`
    // past the brief initial cooldown to reset their budget and resume
    // hammering with the same low backoff.
    //
    // We exercise the post-bug-fix invariant indirectly: the second
    // failure must produce a LARGER backoff than the first (failures=2
    // → 500ms vs failures=1 → 250ms per `backoff_for`). If `/start`
    // wiped the entry, the second failure would re-start at 250ms and
    // be indistinguishable from a fresh attacker.
    //
    // Helper that runs one (start, finish-with-bad-creds) pair.
    async fn one_failed_finish(state: AppState) {
        let r = req("POST", "/api/auth/passkey/login/start")
            .header("origin", ORIGIN)
            .body(Body::empty())
            .unwrap();
        let (_, body) = run_with(state.clone(), r).await;
        let session_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let bad = serde_json::json!({
            "session_id": session_id,
            "credential": {
                "id": "dW5rbm93bg",
                "rawId": "dW5rbm93bg",
                "type": "public-key",
                "extensions": {},
                "response": {
                    "authenticatorData": "ZmFrZQ",
                    "clientDataJSON": "ZmFrZQ",
                    "signature": "ZmFrZQ",
                    "userHandle": null
                }
            }
        });
        let r = req("POST", "/api/auth/passkey/login/finish")
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(bad.to_string()))
            .unwrap();
        let (status, _) = run_with(state, r).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (_tmp, state) = test_state().await;
    let throttle = state.security.throttle().clone();
    let loopback = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

    // First failure → failures=1, backoff ~250ms.
    one_failed_finish(state.clone()).await;

    // Wait past the first lockout. With the bug, the entry would be
    // wiped on the NEXT /start. With the fix, /start preserves it via
    // release_slot.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Second failure (this start+finish pair also exercises the path
    // the bug ran through). After the fix, failures=2 → backoff ~500ms.
    one_failed_finish(state.clone()).await;

    // Peek the lockout window. With the fix, it's ~500ms (backoff_for(2)).
    // With the bug, /start would have wiped between attempts, so this
    // would be ~250ms (backoff_for(1)). Pick a threshold strictly above
    // backoff_for(1)=250ms and well below backoff_for(2)=500ms.
    let lockout = throttle
        .peek(loopback)
        .await
        .expect("post-second-failure throttle entry must exist");
    assert!(
        lockout > Duration::from_millis(350),
        "M1 regression: second failure should produce backoff_for(2)≈500ms, \
         got {lockout:?}. If this is ≤250ms, /login/start is wiping the \
         failure counter between attempts via record_success."
    );
}
