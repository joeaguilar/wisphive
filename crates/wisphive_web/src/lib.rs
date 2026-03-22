mod ws_bridge;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::WebSocket;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;
use tracing::info;

/// Shared server state.
#[derive(Clone)]
struct AppState {
    socket_path: PathBuf,
    config_path: PathBuf,
}

/// Embedded frontend assets (built by Vite into frontend/dist/).
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

/// Serve an embedded static file, falling back to index.html for SPA routing.
async fn static_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        (
            [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
            file.data.to_vec(),
        )
            .into_response()
    } else if let Some(file) = FrontendAssets::get("index.html") {
        Html(std::str::from_utf8(&file.data).unwrap_or("").to_string()).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "not found").into_response()
    }
}

/// WebSocket upgrade handler — bridges browser ↔ daemon.
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state.socket_path))
}

async fn handle_ws(ws: WebSocket, socket_path: PathBuf) {
    if let Err(e) = ws_bridge::bridge(ws, &socket_path).await {
        tracing::warn!("WebSocket bridge error: {e}");
    }
}

/// GET /api/config — read config.json
async fn get_config(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    match std::fs::read_to_string(&state.config_path) {
        Ok(content) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            content,
        ).into_response(),
        Err(_) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            "{}".to_string(),
        ).into_response(),
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
        ).into_response(),
    }
}

/// Start the web server.
pub async fn serve(socket_path: PathBuf, port: u16, dev_mode: bool, host: [u8; 4]) -> anyhow::Result<()> {
    let config_path = socket_path.parent()
        .unwrap_or(std::path::Path::new("."))
        .join("config.json");

    let state = AppState { socket_path, config_path };

    let app = if dev_mode {
        Router::new()
            .route("/ws", get(ws_handler))
            .route("/api/config", get(get_config).put(put_config))
            .with_state(state)
            .layer(
                tower_http::cors::CorsLayer::new()
                    .allow_origin(tower_http::cors::Any)
                    .allow_methods(tower_http::cors::Any)
                    .allow_headers(tower_http::cors::Any),
            )
    } else {
        Router::new()
            .route("/ws", get(ws_handler))
            .route("/api/config", get(get_config).put(put_config))
            .fallback(get(static_handler))
            .with_state(state)
    };

    let addr = SocketAddr::from((host, port));
    info!(%addr, dev_mode, "web server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
