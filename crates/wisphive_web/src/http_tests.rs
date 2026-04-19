//! HTTP-level tests for the security middleware, exercising the real Axum
//! router via `tower::ServiceExt::oneshot`. This lets us build requests with
//! exact Host/Origin/Authorization headers and assert the middleware's
//! response status without standing up a real TCP listener.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::security::SecurityConfig;
use crate::{AppState, build_router};

const HOST: &str = "127.0.0.1:3100";
const ORIGIN: &str = "http://127.0.0.1:3100";
const TOKEN: &str = "test-token-xyz";

fn test_state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let security = SecurityConfig::for_test(
        TOKEN.to_string(),
        vec![ORIGIN.to_string()],
        vec![HOST.to_string()],
    );
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
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
}

async fn run(req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let (_tmp, state) = test_state();
    let app = build_router(state, false);
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
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
        .header("authorization", "Bearer wrong")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_config_accepts_bearer_header() {
    let r = req("GET", "/api/config")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn api_config_accepts_query_token() {
    let r = req("GET", &format!("/api/config?token={TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn evil_origin_is_rejected_on_api() {
    let r = req("GET", "/api/config")
        .header("origin", "http://evil.com")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bad_host_is_rejected_on_api() {
    let r = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("host", "evil.example")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_host_is_rejected() {
    // Axum/Hyper normally synthesize a Host from the URI authority, but if
    // neither the header nor an authority is present we should reject.
    let r = Request::builder()
        .method("GET")
        .uri("/api/config")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn web_token_bootstrap_accessible_without_bearer() {
    let r = req("GET", "/api/web-token").body(Body::empty()).unwrap();
    let (status, body) = run(r).await;
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["token"], TOKEN);
}

#[tokio::test]
async fn web_token_bootstrap_still_origin_gated() {
    let r = req("GET", "/api/web-token")
        .header("origin", "http://evil.com")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
    let r = req("GET", &format!("/ws?token={TOKEN}"))
        .header("origin", "http://evil.com")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ws_with_valid_token_and_origin_passes_middleware() {
    // `oneshot` doesn't carry an `OnUpgrade` extension (only a real hyper
    // server does), so the ws handler itself returns 426 Upgrade Required.
    // What matters for this test is that the middleware let the request
    // through to the handler — i.e. we did *not* get 401/403.
    let r = req("GET", &format!("/ws?token={TOKEN}"))
        .header("origin", ORIGIN)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let (status, _) = run(r).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    assert_ne!(status, StatusCode::FORBIDDEN);
}
