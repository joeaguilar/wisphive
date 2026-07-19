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

/// Maximum bytes a single newline-delimited line from the daemon may occupy
/// before the bridge tears the connection down (itr#83). Without a cap a
/// misbehaving or compromised daemon peer that streams bytes with no newline
/// would grow the line buffer until the web process OOMs. Aligned with the
/// daemon socket reader's 8 MiB cap.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Read one newline-delimited line from the daemon, capping it at
/// [`MAX_LINE_BYTES`] (itr#83). Mirrors `Lines::next_line` semantics
/// (`Ok(None)` at clean EOF, trailing `\n`/`\r\n` stripped) but bounds memory:
/// it pulls from the buffered reader in chunks and errors the moment the
/// accumulated line would exceed the cap, so a peer that never sends a newline
/// can't OOM the bridge.
/// `buf` is the caller-owned partial-line accumulator, kept across calls so the
/// read stays cancel-safe inside `tokio::select!`: if the browser→daemon branch
/// fires and drops this future mid-line, the partial survives in `buf` and the
/// next call resumes it. Cleared on a successful return.
async fn read_capped_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> anyhow::Result<Option<String>> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(idx) => {
                if buf.len() + idx > MAX_LINE_BYTES {
                    return Err(anyhow::anyhow!(
                        "daemon line exceeded {MAX_LINE_BYTES}-byte cap"
                    ));
                }
                buf.extend_from_slice(&available[..idx]);
                reader.consume(idx + 1);
                break;
            }
            None => {
                let take = available.len();
                if buf.len() + take > MAX_LINE_BYTES {
                    return Err(anyhow::anyhow!(
                        "daemon line exceeded {MAX_LINE_BYTES}-byte cap"
                    ));
                }
                buf.extend_from_slice(available);
                reader.consume(take);
            }
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(String::from_utf8(std::mem::take(buf))?))
}

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
    let mut daemon_lines = BufReader::new(reader);
    // Persistent partial-line accumulator for the capped daemon reader (itr#83);
    // kept across select iterations to stay cancel-safe.
    let mut line_buf: Vec<u8> = Vec::new();

    // Handshake with daemon
    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })?;
    daemon_writer.write_all(hello.as_bytes()).await?;

    // Wait for Welcome
    let welcome_line = read_capped_line(&mut daemon_lines, &mut line_buf)
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
            line = read_capped_line(&mut daemon_lines, &mut line_buf) => {
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
                            // Tell the browser its frame was dropped (itr#567)
                            // instead of leaving it waiting on a reply that
                            // will never arrive; the SPA surfaces this in its
                            // error banner. A send failure means the browser
                            // is gone — end the bridge like any other write
                            // failure.
                            if ws_tx
                                .send(Message::Text(malformed_payload_error(text.len()).into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
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
/// Returns `None` if the payload doesn't parse as a ClientCommand — we
/// refuse to forward raw bytes because a compromised browser tab could
/// otherwise emit decisions that look as if they came from a different
/// device. Going through typed decode guarantees the envelope we emit to
/// the daemon always has `device_id = Some(caller)`: any device id the
/// browser embedded is overwritten with the authenticated one.
///
/// A browser-supplied `correlation_id` is preserved (itr#567): it is an
/// opaque echo token scoped to this same connection — the daemon stamps it
/// back onto direct replies (`agent_spawn_queued`, `command_error`) so the
/// SPA can bind an ack or refusal to the exact command it sent. It carries
/// no authority, so forwarding it verbatim is safe.
fn rewrap_with_device(raw: &str, device: &AuthedDevice) -> Option<String> {
    let mut command: ClientCommand = wisphive_protocol::decode(raw).ok()?;
    command.device_id = Some(device.id.clone());
    wisphive_protocol::encode(&command).ok()
}

/// Build the single-frame wire error the bridge sends back to the browser
/// when its payload failed to decode (itr#567). Without this the drop is
/// warn!-only server-side and the browser waits forever for a reply that
/// will never come. The raw payload is deliberately NOT echoed back — it is
/// attacker-influenced and possibly huge; the byte length is enough for the
/// operator to correlate with the server log.
fn malformed_payload_error(len: usize) -> String {
    let frame = ServerMessage::Error {
        message: format!(
            "The daemon bridge dropped a malformed message from this browser ({len} bytes): it did not decode as a Wisphive client command. This is a bug or a stale web client — reload the page."
        ),
    };
    // encode() appends the newline framing used on the Unix socket; the
    // WebSocket is message-framed, so serialize without it.
    serde_json::to_string(&frame).unwrap_or_else(|_| {
        r#"{"type":"error","message":"malformed browser message dropped"}"#.to_string()
    })
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

    // ---- itr#567: browser-supplied correlation ids survive the rewrap ----

    #[test]
    fn rewrap_preserves_browser_correlation_id() {
        let spawn = r#"{"type":"spawn_agent","project":"/proj","prompt":"do it","correlation_id":"spawn-corr-9"}"#;
        let wrapped = rewrap_with_device(spawn, &authed("dev-1")).unwrap();
        let decoded: ClientCommand = wisphive_protocol::decode(&wrapped).unwrap();
        assert_eq!(decoded.correlation_id.as_deref(), Some("spawn-corr-9"));
        // Device attribution still comes from the authenticated middleware,
        // never the payload.
        assert_eq!(
            decoded.device_id.as_ref().map(|d| d.0.as_str()),
            Some("dev-1")
        );
    }

    #[test]
    fn rewrap_without_correlation_id_stays_uncorrelated() {
        let approve = r#"{"type":"approve","id":"00000000-0000-0000-0000-000000000000","always_allow":false}"#;
        let wrapped = rewrap_with_device(approve, &authed("dev-1")).unwrap();
        assert!(!wrapped.contains("correlation_id"));
        let decoded: ClientCommand = wisphive_protocol::decode(&wrapped).unwrap();
        assert!(decoded.correlation_id.is_none());
    }

    // ---- itr#567: a dropped browser frame is answered with a wire error ----

    #[test]
    fn malformed_payload_error_is_a_decodable_error_frame() {
        let frame = malformed_payload_error(42);
        assert!(
            !frame.ends_with('\n'),
            "WebSocket frames are not newline-framed"
        );
        let decoded: ServerMessage = wisphive_protocol::decode(&frame).unwrap();
        match decoded {
            ServerMessage::Error { message } => {
                assert!(
                    message.contains("42 bytes"),
                    "must carry the length: {message}"
                );
            }
            other => panic!("expected Error frame, got {other:?}"),
        }
    }

    // ---- itr#83: capped daemon line length ----

    #[tokio::test]
    async fn capped_reader_accepts_normal_daemon_line() {
        let payload = b"{\"type\":\"welcome\"}\n";
        let mut reader = tokio::io::BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let line = read_capped_line(&mut reader, &mut buf).await.unwrap();
        assert_eq!(line.as_deref(), Some("{\"type\":\"welcome\"}"));
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn capped_reader_rejects_over_limit_daemon_line() {
        // A daemon peer streaming bytes with no newline past the cap must be
        // rejected (bounded memory), not buffered until OOM.
        let oversized = vec![b'x'; MAX_LINE_BYTES + 16];
        let mut reader = tokio::io::BufReader::new(&oversized[..]);
        let mut buf = Vec::new();
        let result = read_capped_line(&mut reader, &mut buf).await;
        assert!(result.is_err(), "over-limit daemon line must error");
        assert!(buf.len() <= MAX_LINE_BYTES);
    }
}
