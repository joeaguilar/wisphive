use std::path::Path;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, warn};
use wisphive_protocol::{
    ClientCommand, ClientMessage, ClientType, PROTOCOL_VERSION, ServerMessage, encode,
};

use crate::security::AuthedDevice;

/// Bridge a WebSocket connection to the daemon's Unix socket.
///
/// 1. Connect to daemon, send Hello(Tui), wait for Welcome.
/// 2. Forward browser→daemon messages and daemon→browser messages bidirectionally.
///
/// Every browser→daemon message is re-wrapped in a [`ClientCommand`]
/// envelope with the authenticated `device_id` attached. That lets the
/// daemon attribute decisions (and eventually gate sudo-class tools) to the
/// originating device without trusting anything the browser tells it about
/// its own identity — the device id comes from the security middleware's
/// token lookup, not the wire.
///
/// A malformed payload from the browser is logged and dropped; we don't
/// forward arbitrary bytes to the daemon because it would let a
/// compromised tab bypass the device_id annotation.
pub async fn bridge(ws: WebSocket, socket_path: &Path, device: AuthedDevice) -> anyhow::Result<()> {
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
                        let Some(payload) = rewrap_with_device(&text, &device) else {
                            warn!(
                                device_id = %device.id,
                                len = text.len(),
                                "dropping malformed browser message"
                            );
                            continue;
                        };
                        debug!(
                            device_id = %device.id,
                            len = payload.len(),
                            "browser → daemon"
                        );
                        if daemon_writer.write_all(payload.as_bytes()).await.is_err() {
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

/// Re-serialize a browser-origin ClientMessage inside a ClientCommand
/// envelope tagged with the authenticated device id.
///
/// Returns `None` if the payload doesn't parse as a ClientMessage — we
/// refuse to forward raw bytes because a compromised browser tab could
/// otherwise emit decisions that look as if they came from a different
/// device. Going through typed decode guarantees the envelope we emit to
/// the daemon always has `device_id = Some(caller)`.
fn rewrap_with_device(raw: &str, device: &AuthedDevice) -> Option<String> {
    let body: ClientMessage = wisphive_protocol::decode(raw).ok()?;
    let command = ClientCommand::from(body).with_device_id(device.id.clone());
    wisphive_protocol::encode(&command).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wisphive_protocol::{ClientCommand, DeviceId};

    fn authed(id: &str) -> AuthedDevice {
        AuthedDevice {
            id: DeviceId(id.to_string()),
            name: "test".into(),
        }
    }

    #[test]
    fn rewrap_attaches_device_id_to_approve() {
        let approve = r#"{"type":"approve","id":"00000000-0000-0000-0000-000000000000","always_allow":false}"#;
        let wrapped = rewrap_with_device(approve, &authed("dev-1")).unwrap();
        assert!(wrapped.contains("\"device_id\":\"dev-1\""));
        let decoded: ClientCommand = wisphive_protocol::decode(&wrapped).unwrap();
        assert_eq!(
            decoded.device_id.as_ref().map(|d| d.0.as_str()),
            Some("dev-1")
        );
    }

    #[test]
    fn rewrap_overwrites_attacker_provided_device_id() {
        // A malicious tab could try to spoof a different device id. The
        // rewrap must throw theirs away and use the authenticated one.
        let spoof = r#"{"type":"approve","id":"00000000-0000-0000-0000-000000000000","always_allow":false,"device_id":"attacker"}"#;
        let wrapped = rewrap_with_device(spoof, &authed("real-device")).unwrap();
        assert!(wrapped.contains("\"device_id\":\"real-device\""));
        assert!(!wrapped.contains("\"attacker\""));
    }

    #[test]
    fn rewrap_rejects_garbage() {
        assert!(rewrap_with_device("not json at all", &authed("d")).is_none());
        assert!(rewrap_with_device(r#"{"type":"not_a_variant"}"#, &authed("d")).is_none());
    }
}
