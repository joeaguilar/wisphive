//! One-shot IPC call used by `POST /api/auth/reauth` to tell the daemon to
//! refresh a device's sudo-mode freshness.
//!
//! The web crate could "just" share an `Arc<ReauthRegistry>` with the daemon
//! when they live in the same process (`wisphive daemon start --web`), but
//! `wisphive web` can also run standalone against a separate daemon — in
//! that case the only communication channel is the Unix socket. Using the
//! socket in both modes keeps the trust boundary identical: the daemon is
//! the only thing that decides "this device is fresh."
//!
//! The protocol is a three-line dance:
//! 1. `{"type":"hello", "client":"tui", "version": N}` so the daemon dispatches
//!    us to `handle_tui`.
//! 2. `ClientCommand { body: MarkDeviceFresh, device_id: Some(authed) }`.
//!    The envelope's `device_id` is the authenticated device — the daemon
//!    trusts the sender of this socket (same-host Unix socket) and reads the
//!    id straight off the envelope.
//! 3. Read frames until we see `MarkDeviceFreshAck { device_id }`; anything
//!    else (Welcome, AgentsSnapshot, QueueSnapshot, etc.) is ignored.
//!
//! A hard timeout guards step 3 so a hung daemon can't pin an HTTP request.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::warn;
use wisphive_protocol::{
    ClientCommand, ClientMessage, ClientType, DeviceId, PROTOCOL_VERSION, ServerMessage, encode,
};

/// Upper bound on the round-trip. The daemon's TUI loop runs tightly —
/// under a millisecond on the happy path. 3 s gives us plenty of slack for
/// a loaded host while still bounding a stuck daemon.
const REAUTH_IPC_DEADLINE: Duration = Duration::from_secs(3);

/// Outcome of a single [`signal_mark_device_fresh`] call. Separating the
/// error cases makes the handler's response mapping explicit instead of
/// stringifying anyhow errors.
#[derive(Debug, thiserror::Error)]
pub enum ReauthIpcError {
    #[error("failed to connect to daemon: {0}")]
    Connect(#[source] std::io::Error),
    #[error("failed to send command to daemon: {0}")]
    Write(#[source] std::io::Error),
    #[error("daemon closed connection before ack")]
    ClosedBeforeAck,
    #[error("reached deadline waiting for ack")]
    Timeout,
    #[error("daemon acked for different device id: expected {expected}, got {got}")]
    WrongDevice { expected: String, got: String },
}

/// Open a short-lived Tui connection to the daemon, send
/// `MarkDeviceFresh` for `device_id`, wait for the ack, and close.
///
/// Returns `Ok(())` only after the ack has been observed — the caller can
/// then safely 200 back to the browser knowing a follow-up Approve will
/// see the refreshed freshness timestamp.
pub async fn signal_mark_device_fresh(
    socket_path: &Path,
    device_id: &DeviceId,
) -> Result<(), ReauthIpcError> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(ReauthIpcError::Connect)?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // (1) Hello — encoded as a bare ClientMessage; on the wire this is
    // identical to ClientCommand { body: Hello, device_id: None } because
    // of #[serde(flatten)] + skip_serializing_if.
    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })
    .expect("ClientMessage::Hello always serializes");
    writer
        .write_all(hello.as_bytes())
        .await
        .map_err(ReauthIpcError::Write)?;

    // (2) MarkDeviceFresh wrapped with the authenticated device id.
    let command =
        ClientCommand::from(ClientMessage::MarkDeviceFresh).with_device_id(device_id.clone());
    let encoded = encode(&command).expect("MarkDeviceFresh always serializes");
    writer
        .write_all(encoded.as_bytes())
        .await
        .map_err(ReauthIpcError::Write)?;

    // (3) Read until ack or deadline. The daemon will emit Welcome,
    // AgentsSnapshot, and QueueSnapshot on the way — all fine to ignore.
    // A decode error on an intermediate line also doesn't stop us; we
    // just keep reading until we see the ack.
    let wait_for_ack = async {
        loop {
            let text = match lines.next_line().await {
                Ok(Some(t)) => t,
                Ok(None) => return Err(ReauthIpcError::ClosedBeforeAck),
                Err(e) => return Err(ReauthIpcError::Write(e)),
            };
            match wisphive_protocol::decode::<ServerMessage>(&text) {
                Ok(ServerMessage::MarkDeviceFreshAck { device_id: acked }) => {
                    return Ok(acked);
                }
                Ok(_) => continue,
                Err(e) => {
                    warn!(error = %e, "ignoring undecodable line from daemon while awaiting ack");
                    continue;
                }
            }
        }
    };

    let acked = match tokio::time::timeout(REAUTH_IPC_DEADLINE, wait_for_ack).await {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(ReauthIpcError::Timeout),
    };
    if acked != device_id.0 {
        return Err(ReauthIpcError::WrongDevice {
            expected: device_id.0.clone(),
            got: acked,
        });
    }

    // Close cleanly. Ignoring the error is fine — we already got the ack
    // we care about.
    let _ = writer.shutdown().await;
    Ok(())
}
