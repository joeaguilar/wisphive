//! HTTP-level tests for the security middleware, exercising the real Axum
//! router via `tower::ServiceExt::oneshot`. This lets us build requests with
//! exact Host/Origin/Authorization headers and assert the middleware's
//! response status without standing up a real TCP listener.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tower::ServiceExt;
use wisphive_daemon::state::StateDb;
use wisphive_protocol::{ClientCommand, ClientMessage, PROTOCOL_VERSION, ServerMessage, encode};

use crate::auth::{generate_device_token, hash_password};
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
    };
    let r = req("GET", "/api/auth/status").body(Body::empty()).unwrap();
    let (status, _) = run_with(state, r).await;
    assert_eq!(status, StatusCode::OK);
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
