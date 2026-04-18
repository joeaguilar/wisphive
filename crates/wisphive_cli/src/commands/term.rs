//! `wisphive term` CLI subcommands.
//!
//! `new`/`list`/`attach`/`replay`/`close` — a thin client that shells out to
//! the daemon's terminal session manager over the Unix socket and, for
//! `attach`/`new --attach`, hands the user's current terminal over to the
//! remote PTY via a raw-mode stdin/stdout proxy.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use crossterm::terminal;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;
use wisphive_daemon::DaemonConfig;
use wisphive_protocol::{
    ClientMessage, ClientType, PROTOCOL_VERSION, ServerMessage, TerminalSessionMeta, TerminalStatus,
    encode,
};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Raw-mode guard: restore the terminal on Drop even if the program panics.
struct RawGuard;

impl RawGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

async fn connect() -> Result<(
    tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    tokio::net::unix::OwnedWriteHalf,
)> {
    let config = DaemonConfig::default_location();
    let stream = UnixStream::connect(&config.socket_path)
        .await
        .context("could not connect to daemon — is it running? (wisphive daemon start)")?;
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })?;
    write_half.write_all(hello.as_bytes()).await?;

    // Consume Welcome + AgentsSnapshot + QueueSnapshot that handle_tui always sends.
    for _ in 0..3 {
        let _ = lines.next_line().await?;
    }
    Ok((lines, write_half))
}

/// `wisphive term list`
pub async fn list() -> Result<()> {
    let (mut lines, mut writer) = connect().await?;
    writer
        .write_all(encode(&ClientMessage::TermList)?.as_bytes())
        .await?;

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match wisphive_protocol::decode(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let ServerMessage::TermListResponse { sessions } = msg {
            print_list(&sessions);
            return Ok(());
        }
    }
    anyhow::bail!("daemon closed before responding to term list")
}

fn print_list(sessions: &[TerminalSessionMeta]) {
    if sessions.is_empty() {
        println!("no terminal sessions");
        return;
    }
    println!("{:<36}  {:<10}  {:<20}  COMMAND", "ID", "STATUS", "LABEL");
    for s in sessions {
        println!(
            "{:<36}  {:<10}  {:<20}  {} {}",
            s.id,
            s.status,
            s.label.as_deref().unwrap_or("-"),
            s.command,
            s.args.join(" ")
        );
    }
}

/// `wisphive term new [--label X] [--cwd DIR] [--cmd CMD] [--attach]`
pub async fn new_session(
    label: Option<String>,
    cwd: Option<PathBuf>,
    command: Option<String>,
    args: Option<Vec<String>>,
    attach_after: bool,
) -> Result<()> {
    let (cols, rows) = current_term_size();
    let (mut lines, mut writer) = connect().await?;

    writer
        .write_all(
            encode(&ClientMessage::TermCreate {
                label,
                command,
                args,
                cwd,
                cols,
                rows,
                env: None,
            })?
            .as_bytes(),
        )
        .await?;

    // Wait for TermCreated.
    let meta = loop {
        let Some(line) = lines.next_line().await? else {
            anyhow::bail!("daemon closed before creating terminal");
        };
        let msg: ServerMessage = wisphive_protocol::decode(&line)?;
        match msg {
            ServerMessage::TermCreated(m) => break m,
            ServerMessage::TermError { message, .. } => {
                anyhow::bail!("create failed: {message}");
            }
            _ => {}
        }
    };

    println!("created terminal session {}", meta.id);

    if attach_after {
        attach_loop(meta.id, lines, writer).await?;
    }
    Ok(())
}

/// `wisphive term attach <id>`
pub async fn attach(id_str: String) -> Result<()> {
    let id = Uuid::parse_str(&id_str).context("invalid session id")?;
    let (lines, writer) = connect().await?;
    attach_loop(id, lines, writer).await
}

async fn attach_loop(
    id: Uuid,
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut writer: tokio::net::unix::OwnedWriteHalf,
) -> Result<()> {
    writer
        .write_all(encode(&ClientMessage::TermAttach { id })?.as_bytes())
        .await?;

    let _raw = RawGuard::enter()?;
    let mut stdout = io::stdout();
    let exit_flag = Arc::new(AtomicBool::new(false));

    // Spawn a blocking stdin reader thread that pushes keystrokes into a
    // channel. We can't poll stdin with tokio portably, so we do it the
    // blocking way and bridge via an mpsc.
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let exit_flag_reader = exit_flag.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        while !exit_flag_reader.load(Ordering::Relaxed) {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        tokio::select! {
            msg = lines.next_line() => {
                match msg? {
                    Some(line) => {
                        let decoded: ServerMessage = match wisphive_protocol::decode(&line) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        match decoded {
                            ServerMessage::TermCatchup { id: sid, screen, .. } if sid == id => {
                                if let Ok(bytes) = B64.decode(&screen) {
                                    stdout.write_all(&bytes)?;
                                    stdout.flush()?;
                                }
                            }
                            ServerMessage::TermChunk { id: sid, data, direction, .. }
                                if sid == id && matches!(direction, wisphive_protocol::TerminalDirection::Output) =>
                            {
                                if let Ok(bytes) = B64.decode(&data) {
                                    stdout.write_all(&bytes)?;
                                    stdout.flush()?;
                                }
                            }
                            ServerMessage::TermEnded { id: sid, .. } if sid == id => {
                                exit_flag.store(true, Ordering::Relaxed);
                                break;
                            }
                            ServerMessage::TermError { message, .. } => {
                                eprintln!("\r\nwisphive: terminal error: {message}\r\n");
                            }
                            _ => {}
                        }
                    }
                    None => break,
                }
            }
            bytes = stdin_rx.recv() => {
                let Some(bytes) = bytes else { break };
                // Ctrl-] (0x1d) detaches cleanly without sending to PTY.
                if bytes.contains(&0x1d) {
                    exit_flag.store(true, Ordering::Relaxed);
                    let _ = writer.write_all(encode(&ClientMessage::TermDetach { id })?.as_bytes()).await;
                    break;
                }
                let data = B64.encode(&bytes);
                writer.write_all(encode(&ClientMessage::TermInput { id, data })?.as_bytes()).await?;
            }
        }
    }
    Ok(())
}

/// `wisphive term replay <id> [--speed N]`
pub async fn replay(id_str: String, speed: f32) -> Result<()> {
    let id = Uuid::parse_str(&id_str).context("invalid session id")?;
    let (mut lines, mut writer) = connect().await?;
    writer
        .write_all(
            encode(&ClientMessage::TermReplay {
                id,
                from_seq: None,
                speed: Some(speed),
            })?
            .as_bytes(),
        )
        .await?;

    let mut last_ts: Option<i64> = None;
    let mut stdout = io::stdout();
    while let Some(line) = lines.next_line().await? {
        let decoded: ServerMessage = match wisphive_protocol::decode(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        match decoded {
            ServerMessage::TermReplayChunk {
                id: sid,
                ts_us,
                direction,
                data,
                ..
            } if sid == id => {
                // Pace playback according to the recorded timestamps.
                if let Some(last) = last_ts {
                    let delta_us = (ts_us - last).max(0) as f64 / speed as f64;
                    if delta_us > 0.0 {
                        tokio::time::sleep(Duration::from_micros(delta_us as u64)).await;
                    }
                }
                last_ts = Some(ts_us);
                if matches!(direction, wisphive_protocol::TerminalDirection::Output)
                    && let Ok(bytes) = B64.decode(&data)
                {
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
            }
            ServerMessage::TermReplayDone { id: sid, total_events } if sid == id => {
                println!("\r\n[replay complete: {total_events} events]\r");
                return Ok(());
            }
            ServerMessage::TermError { message, .. } => {
                anyhow::bail!("replay failed: {message}");
            }
            _ => {}
        }
    }
    Ok(())
}

/// `wisphive term close <id> [--kill]`
pub async fn close(id_str: String, kill: bool) -> Result<()> {
    let id = Uuid::parse_str(&id_str).context("invalid session id")?;
    let (mut _lines, mut writer) = connect().await?;
    writer
        .write_all(encode(&ClientMessage::TermClose { id, kill })?.as_bytes())
        .await?;
    println!("close request sent");
    Ok(())
}

fn current_term_size() -> (u16, u16) {
    terminal::size().unwrap_or((120, 32))
}

// Silence clippy: TerminalStatus import is used via fully-qualified path in
// tests. Keep a dead reference so unused-imports does not fire on release.
#[allow(dead_code)]
fn _keep_terminal_status_in_scope() -> TerminalStatus {
    TerminalStatus::Running
}
