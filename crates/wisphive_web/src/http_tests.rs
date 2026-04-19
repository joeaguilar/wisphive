//! HTTP-level tests for the security middleware, exercising the real Axum
//! router via `tower::ServiceExt::oneshot`. This lets us build requests with
//! exact Host/Origin/Authorization headers and assert the middleware's
//! response status without standing up a real TCP listener.

use std::net::{IpAddr, Ipv4Addr};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use wisphive_daemon::state::StateDb;

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

#[tokio::test]
async fn login_when_no_password_set_returns_401_without_leaking_state() {
    // Don't use the default test_state helper — start from a fresh DB with
    // no password row so the handler's "no hash" branch is exercised.
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
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Body must not mention "no password set" or similar — info leak.
    let s = String::from_utf8_lossy(&body).to_lowercase();
    assert!(!s.contains("no password"));
    assert!(!s.contains("not set"));
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
