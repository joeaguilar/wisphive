use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wisphive_protocol::{ClientMessage, ClientType, PROTOCOL_VERSION, ServerMessage, encode};

/// Async connection to the Wisphive daemon.
pub struct DaemonConnection {
    reader: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: tokio::net::unix::OwnedWriteHalf,
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

    /// Read the next message from the daemon. Returns None on disconnect.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>> {
        match self.reader.next_line().await? {
            Some(line) => {
                let msg: ServerMessage = wisphive_protocol::decode(&line)?;
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    /// Send a command to the daemon.
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let encoded = encode(msg)?;
        self.writer.write_all(encoded.as_bytes()).await?;
        Ok(())
    }
}
