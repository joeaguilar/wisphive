use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wisphive_protocol::{ClientMessage, ClientType, PROTOCOL_VERSION, ServerMessage, encode};

/// Async connection to the Wisphive daemon.
pub struct DaemonConnection {
    reader: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

#[derive(Debug, PartialEq, Eq)]
struct DecodeFailureMetadata {
    category: &'static str,
    line: usize,
    column: usize,
    frame_bytes: usize,
}

fn decode_failure_metadata(error: &serde_json::Error, frame: &str) -> DecodeFailureMetadata {
    let category = match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    };
    DecodeFailureMetadata {
        category,
        line: error.line(),
        column: error.column(),
        frame_bytes: frame.len(),
    }
}

impl DaemonConnection {
    /// Connect to the daemon and perform the handshake.
    pub async fn connect(socket_path: &std::path::Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await?;
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half).lines();

        // Send hello
        let hello = encode(&ClientMessage::Hello {
            client: ClientType::Tui,
            version: PROTOCOL_VERSION,
        })?;
        write_half.write_all(hello.as_bytes()).await?;

        // Read welcome
        let welcome_line = reader
            .next_line()
            .await?
            .ok_or_else(|| anyhow::anyhow!("daemon closed connection during handshake"))?;
        let welcome: ServerMessage = wisphive_protocol::decode(&welcome_line)?;

        match welcome {
            ServerMessage::Welcome { .. } => {}
            ServerMessage::Error { message } => {
                anyhow::bail!("daemon rejected connection: {message}");
            }
            _ => {
                anyhow::bail!("unexpected handshake response");
            }
        }

        Ok(Self {
            reader,
            writer: write_half,
        })
    }

    /// Read the next known message from the daemon. Returns `None` on a clean
    /// disconnect. Malformed lines and additive message variants unknown to
    /// this client are logged and skipped without disturbing later frames.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>> {
        loop {
            let Some(line) = self.reader.next_line().await? else {
                return Ok(None);
            };

            match wisphive_protocol::decode(&line) {
                Ok(message) => return Ok(Some(message)),
                Err(error) => {
                    // serde_json's Display text includes attacker-controlled
                    // enum tags and can contain newlines. Log fixed labels and
                    // numbers only; never the raw frame or error text.
                    let metadata = decode_failure_metadata(&error, &line);
                    tracing::warn!(
                        decode_category = metadata.category,
                        decode_line = metadata.line,
                        decode_column = metadata.column,
                        frame_bytes = metadata.frame_bytes,
                        "skipping unknown or malformed daemon message"
                    );
                }
            }
        }
    }

    /// Send a command to the daemon.
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let encoded = encode(msg)?;
        self.writer.write_all(encoded.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection_from_stream(stream: UnixStream) -> DaemonConnection {
        let (read_half, write_half) = stream.into_split();
        DaemonConnection {
            reader: BufReader::new(read_half).lines(),
            writer: write_half,
        }
    }

    #[tokio::test]
    async fn recv_skips_adversarial_and_garbled_frames_without_reordering_known_messages() {
        let (client, mut daemon) = UnixStream::pair().unwrap();
        let mut connection = connection_from_stream(client);

        let attacker_marker = "FORGED_LOG_ENTRY level=ERROR";
        let unknown_tag = format!("future_{}\n{attacker_marker}", "x".repeat(64 * 1024));
        let unknown_frame = serde_json::to_string(&serde_json::json!({
            "type": unknown_tag,
            "sequence": 1,
        }))
        .unwrap();
        assert!(unknown_frame.contains(&format!("\\n{attacker_marker}")));

        let error = wisphive_protocol::decode::<ServerMessage>(&unknown_frame).unwrap_err();
        let metadata = decode_failure_metadata(&error, &unknown_frame);
        let rendered_metadata = format!("{metadata:?}");
        assert_eq!(metadata.category, "data");
        assert_eq!(metadata.frame_bytes, unknown_frame.len());
        assert!(!rendered_metadata.contains(attacker_marker));
        assert!(rendered_metadata.len() < 128);

        // Write concurrently with recv: the adversarial frame is deliberately
        // larger than common Unix-socket buffers and must not deadlock the
        // test before the reader starts draining it.
        let writer = tokio::spawn(async move {
            daemon.write_all(unknown_frame.as_bytes()).await.unwrap();
            daemon.write_all(b"\n").await.unwrap();
            daemon.write_all(b"not-json\n").await.unwrap();
            daemon
                .write_all(
                    encode(&ServerMessage::Error {
                        message: "first".into(),
                    })
                    .unwrap()
                    .as_bytes(),
                )
                .await
                .unwrap();
            daemon.write_all(b"{\"type\":\n").await.unwrap();
            daemon
                .write_all(
                    encode(&ServerMessage::ReimportComplete { count: 2 })
                        .unwrap()
                        .as_bytes(),
                )
                .await
                .unwrap();
            daemon.shutdown().await.unwrap();
        });

        assert!(matches!(
            connection.recv().await.unwrap(),
            Some(ServerMessage::Error { message }) if message == "first"
        ));
        assert!(matches!(
            connection.recv().await.unwrap(),
            Some(ServerMessage::ReimportComplete { count: 2 })
        ));
        assert!(connection.recv().await.unwrap().is_none());
        writer.await.unwrap();
    }
}
