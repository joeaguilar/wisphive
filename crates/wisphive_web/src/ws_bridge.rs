use std::path::Path;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, warn};
use wisphive_protocol::{ClientMessage, ClientType, PROTOCOL_VERSION, ServerMessage, encode};

/// Bridge a WebSocket connection to the daemon's Unix socket.
///
/// 1. Connect to daemon, send Hello(Tui), wait for Welcome.
/// 2. Forward browser→daemon messages and daemon→browser messages bidirectionally.
pub async fn bridge(ws: WebSocket, socket_path: &Path) -> anyhow::Result<()> {
    // Connect to daemon
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut daemon_writer) = stream.into_split();
    let mut daemon_lines = BufReader::new(reader).lines();

    // Handshake with daemon
    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })?;
    daemon_writer.write_all(hello.as_bytes()).await?;

    // Wait for Welcome
    let welcome_line = daemon_lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow::anyhow!("daemon closed before welcome"))?;
    let _welcome: ServerMessage = wisphive_protocol::decode(&welcome_line)?;

    // Split WebSocket
    let (mut ws_tx, mut ws_rx) = ws.split();

    // Send the Welcome to the browser
    ws_tx.send(Message::Text(welcome_line.into())).await?;

    // Bidirectional bridge
    loop {
        tokio::select! {
            // Daemon → Browser
            line = daemon_lines.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        debug!(len = text.len(), "daemon → browser");
                        if ws_tx.send(Message::Text(text.into())).await.is_err() {
                            break; // Browser disconnected
                        }
                    }
                    Ok(None) => break, // Daemon closed
                    Err(e) => {
                        warn!("daemon read error: {e}");
                        break;
                    }
                }
            }
            // Browser → Daemon
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!(len = text.len(), "browser → daemon");
                        let mut line = text.to_string();
                        if !line.ends_with('\n') {
                            line.push('\n');
                        }
                        if daemon_writer.write_all(line.as_bytes()).await.is_err() {
                            break; // Daemon closed
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("websocket error: {e}");
                        break;
                    }
                    _ => {} // Ping/Pong handled by axum
                }
            }
        }
    }

    debug!("WebSocket bridge closed");
    Ok(())
}
