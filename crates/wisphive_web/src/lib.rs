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

/// Embedded frontend assets (built by Vite into frontend/dist/).
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

/// Serve an embedded static file, falling back to index.html for SPA routing.
async fn static_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first
    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        (
            [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
            file.data.to_vec(),
        )
            .into_response()
    } else if let Some(file) = FrontendAssets::get("index.html") {
        // SPA fallback
        Html(std::str::from_utf8(&file.data).unwrap_or("").to_string()).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "not found").into_response()
    }
}

/// WebSocket upgrade handler — bridges browser ↔ daemon.
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(socket_path): axum::extract::State<PathBuf>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, socket_path))
}

async fn handle_ws(ws: WebSocket, socket_path: PathBuf) {
    if let Err(e) = ws_bridge::bridge(ws, &socket_path).await {
        tracing::warn!("WebSocket bridge error: {e}");
    }
}

/// Start the web server.
///
/// - `socket_path`: path to the daemon's Unix socket
/// - `port`: HTTP port to listen on
/// - `dev_mode`: if true, proxy to Vite dev server instead of serving embedded assets
pub async fn serve(socket_path: PathBuf, port: u16, dev_mode: bool) -> anyhow::Result<()> {
    let app = if dev_mode {
        // In dev mode, only serve the WebSocket endpoint.
        // The Vite dev server handles static assets (run `npm run dev` separately).
        Router::new()
            .route("/ws", get(ws_handler))
            .with_state(socket_path)
            .layer(
                tower_http::cors::CorsLayer::new()
                    .allow_origin(tower_http::cors::Any)
                    .allow_methods(tower_http::cors::Any)
                    .allow_headers(tower_http::cors::Any),
            )
    } else {
        // Production: serve embedded frontend + WebSocket
        Router::new()
            .route("/ws", get(ws_handler))
            .fallback(get(static_handler))
            .with_state(socket_path)
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!(%addr, dev_mode, "web server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
