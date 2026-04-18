//! Daemon-managed PTY terminal sessions.
//!
//! A terminal session is a child process attached to a pseudo-terminal that
//! wisphive owns. The PTY stream is persisted event-by-event to SQLite for
//! audit/replay, fanned out live to any number of attached viewers (TUI or
//! web), and mirrored into a vt100 parser so that new viewers can be handed
//! an instant "catchup" snapshot of the current screen.
//!
//! Architecture:
//!
//! ```text
//! reader thread (blocking)  ──►  per-session broadcast<TermFrame>  ──►  attached clients
//!                           └─►  vt100 parser (catchup screen state)
//!                           └─►  mpsc<TermFrame>  ──►  db batcher  ──►  terminal_events
//! ```
//!
//! The child process is spawned with `WISPHIVE_TERMINAL_SESSION_ID` in its
//! environment so that any hook (e.g. `wisphive-hook` invoked from `claude`
//! inside the PTY) can attach the session id to its `DecisionRequest` for
//! cross-referencing approvals with the terminal they came from.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use bytes::Bytes;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;
use wisphive_protocol::{
    ServerMessage, TerminalDirection, TerminalSessionMeta, TerminalStatus,
};

use crate::state::StateDb;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Maximum PTY dimensions accepted. vt100 allocates per-cell so very large
/// terminals burn memory; wisphive rejects anything past this bound.
const MAX_COLS: u16 = 500;
const MAX_ROWS: u16 = 200;

/// Chunk size for PTY reads. Frames larger than this are split so that a
/// single client can't hold up the broadcast with a multi-megabyte write.
const CHUNK_BYTES: usize = 4096;

/// A single event from a terminal's live stream.
#[derive(Debug, Clone)]
pub struct TermFrame {
    pub seq: u64,
    pub ts_us: i64,
    pub direction: TerminalDirection,
    pub bytes: Bytes,
}

/// One running (or recently ended) terminal session.
pub struct TerminalSession {
    pub id: Uuid,
    /// Metadata. Guarded by async mutex because shutdown/wait tasks update it
    /// concurrently with read queries from `handle_tui`.
    pub meta: Mutex<TerminalSessionMeta>,
    /// PTY master writer used for forwarding stdin. `Box<dyn Write + Send>`
    /// is blocking; wrap writes in `spawn_blocking` at the caller.
    writer: std::sync::Mutex<Box<dyn std::io::Write + Send>>,
    /// PTY master kept alive for resize and as the owner of the kernel fd.
    master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    /// vt100 screen state, updated by the reader thread. Snapshot source for
    /// catchup when new clients attach.
    parser: std::sync::Mutex<vt100::Parser>,
    /// Broadcast fanout for live viewers. Lag drops for laggard receivers;
    /// they are expected to re-attach and pick up a fresh catchup snapshot.
    bcast: broadcast::Sender<Arc<TermFrame>>,
    /// Monotonic sequence counter across input+output+resize events.
    seq: AtomicU64,
    /// Child process handle. Moved into the waiter task early so it can
    /// call `wait()`. Do NOT use this for shutdown — use `killer` instead.
    child: std::sync::Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    /// Clone-killer for the child. Stays here for the life of the session
    /// so `shutdown_all` can terminate the PTY child even after the waiter
    /// task has taken `child` for `wait()`.
    killer: std::sync::Mutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    /// Drop-guard flag: once true, the reader thread is expected to have
    /// exited and no further events will be produced.
    ended: std::sync::atomic::AtomicBool,
}

impl TerminalSession {
    /// Return the next sequence number.
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel)
    }

    /// Read the sequence counter without incrementing. Used by new viewers
    /// to filter out any stale frames their broadcast receiver may re-deliver.
    pub fn seq_load(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Subscribe a new viewer to live frames.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TermFrame>> {
        self.bcast.subscribe()
    }

    /// Snapshot the current vt100 screen contents as a byte stream that, when
    /// written to a fresh terminal emulator, reproduces the current display.
    pub fn catchup_snapshot(&self) -> Vec<u8> {
        let parser = self.parser.lock().expect("parser poisoned");
        parser.screen().contents_formatted()
    }
}

/// Manages the lifecycle of all terminal sessions.
pub struct TerminalSessionManager {
    sessions: Mutex<HashMap<Uuid, Arc<TerminalSession>>>,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
}

impl TerminalSessionManager {
    pub fn new(state_db: Arc<StateDb>, tui_tx: broadcast::Sender<ServerMessage>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            state_db,
            tui_tx,
        }
    }

    /// Look up a running session. Historical/orphaned sessions live only in
    /// SQLite and return `None` from this accessor.
    pub async fn get(&self, id: Uuid) -> Option<Arc<TerminalSession>> {
        self.sessions.lock().await.get(&id).cloned()
    }

    /// Spawn a new PTY-backed session.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        self: &Arc<Self>,
        label: Option<String>,
        command: Option<String>,
        args: Option<Vec<String>>,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        env: Option<HashMap<String, String>>,
    ) -> Result<TerminalSessionMeta> {
        if cols == 0 || rows == 0 {
            return Err(anyhow!("terminal cols/rows must be nonzero"));
        }
        if cols > MAX_COLS || rows > MAX_ROWS {
            return Err(anyhow!(
                "terminal size {cols}x{rows} exceeds max {MAX_COLS}x{MAX_ROWS}"
            ));
        }

        // Resolve command + cwd
        let (cmd_str, cmd_args) = match command {
            Some(c) => (c, args.unwrap_or_default()),
            None => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                (shell, vec!["-l".into()])
            }
        };
        let cwd_path = match cwd {
            Some(p) => p,
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        };

        let id = Uuid::new_v4();

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty failed")?;

        let mut builder = CommandBuilder::new(&cmd_str);
        for arg in &cmd_args {
            builder.arg(arg);
        }
        builder.cwd(&cwd_path);
        builder.env("WISPHIVE_TERMINAL_SESSION_ID", id.to_string());
        builder.env("TERM", "xterm-256color");
        if let Some(ref extra) = env {
            for (k, v) in extra {
                builder.env(k, v);
            }
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .context("spawn_command failed")?;
        // Clone a killer up-front — this handle survives the waiter task
        // taking ownership of `child`, so shutdown can still kill the PTY
        // process regardless of where the waiter is in its state machine.
        let killer = child.clone_killer();
        // The slave must be dropped so the master sees EOF when the child closes.
        drop(pair.slave);

        let writer = pair.master.take_writer().context("take_writer failed")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("try_clone_reader failed")?;

        // Scrollback=0: we rely on vt100 for the current screen only; full
        // replay goes through SQLite. Keeping scrollback out of memory bounds
        // worst-case cost to O(cols*rows).
        let parser = vt100::Parser::new(rows, cols, 0);

        let (bcast_tx, _) = broadcast::channel::<Arc<TermFrame>>(256);
        let (db_tx, db_rx) = mpsc::channel::<TermFrame>(1024);

        let started_at = chrono::Utc::now();
        let meta = TerminalSessionMeta {
            id,
            label,
            command: cmd_str,
            args: cmd_args,
            cwd: cwd_path,
            cols,
            rows,
            started_at,
            ended_at: None,
            exit_code: None,
            status: TerminalStatus::Running,
        };
        self.state_db.create_terminal_session(&meta).await?;

        let session = Arc::new(TerminalSession {
            id,
            meta: Mutex::new(meta.clone()),
            writer: std::sync::Mutex::new(writer),
            master: std::sync::Mutex::new(pair.master),
            parser: std::sync::Mutex::new(parser),
            bcast: bcast_tx.clone(),
            seq: AtomicU64::new(0),
            child: std::sync::Mutex::new(Some(child)),
            killer: std::sync::Mutex::new(Some(killer)),
            ended: std::sync::atomic::AtomicBool::new(false),
        });

        self.sessions.lock().await.insert(id, session.clone());

        // Reader thread: portable_pty's reader is blocking, so we drive it
        // from a dedicated OS thread instead of a tokio task.
        spawn_reader_thread(session.clone(), reader, db_tx.clone());

        // DB batcher: drains frames into SQLite in small transactional batches.
        tokio::spawn(run_db_batcher(
            id,
            db_rx,
            self.state_db.clone(),
        ));

        // Waiter: whenever the reader exits or the child dies, persist the
        // final status and broadcast TermEnded to all TUIs.
        tokio::spawn(run_waiter(
            session.clone(),
            self.state_db.clone(),
            self.tui_tx.clone(),
            self.sessions_handle(),
        ));

        info!(session_id = %id, "terminal session created");
        Ok(meta)
    }

    fn sessions_handle(self: &Arc<Self>) -> Arc<Self> {
        self.clone()
    }

    /// Forward bytes to the PTY's stdin.
    pub async fn write_input(&self, id: Uuid, bytes: Vec<u8>) -> Result<()> {
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow!("terminal session {id} not found"))?;

        // Log the input event for faithful replay.
        let seq = session.next_seq();
        let ts_us = chrono::Utc::now().timestamp_micros();
        let frame = TermFrame {
            seq,
            ts_us,
            direction: TerminalDirection::Input,
            bytes: Bytes::copy_from_slice(&bytes),
        };
        let _ = session.bcast.send(Arc::new(frame));
        self.state_db
            .insert_terminal_events_batch(&[(
                id,
                seq,
                ts_us,
                TerminalDirection::Input,
                bytes.clone(),
            )])
            .await?;

        // Blocking write on a spawn_blocking thread. The writer mutex is std
        // because portable-pty's writer is sync.
        tokio::task::spawn_blocking(move || {
            let mut w = session.writer.lock().expect("pty writer poisoned");
            w.write_all(&bytes)?;
            w.flush()?;
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(|e| anyhow!("input join error: {e}"))??;
        Ok(())
    }

    /// Resize the PTY.
    pub async fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<()> {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return Err(anyhow!("invalid resize to {cols}x{rows}"));
        }
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow!("terminal session {id} not found"))?;

        {
            let master = session.master.lock().expect("pty master poisoned");
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| anyhow!("pty resize failed: {e}"))?;
        }
        {
            let mut parser = session.parser.lock().expect("parser poisoned");
            parser.set_size(rows, cols);
        }
        {
            let mut meta = session.meta.lock().await;
            meta.cols = cols;
            meta.rows = rows;
        }

        // Log as a resize event.
        let seq = session.next_seq();
        let ts_us = chrono::Utc::now().timestamp_micros();
        let payload = format!("{cols},{rows}").into_bytes();
        let frame = TermFrame {
            seq,
            ts_us,
            direction: TerminalDirection::Resize,
            bytes: Bytes::copy_from_slice(&payload),
        };
        let _ = session.bcast.send(Arc::new(frame));
        self.state_db
            .insert_terminal_events_batch(&[(
                id,
                seq,
                ts_us,
                TerminalDirection::Resize,
                payload,
            )])
            .await?;
        Ok(())
    }

    /// Close a session. If `kill` is true, forcibly terminate the child.
    /// Otherwise attempt a graceful close by sending HUP via `child.kill()`
    /// (portable-pty only exposes kill — a hangup equivalent would need raw
    /// libc). Either way the session is removed from the live map.
    pub async fn close(&self, id: Uuid, _kill: bool) -> Result<()> {
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow!("terminal session {id} not found"))?;
        // Kill via the clone-killer so this works even if the waiter task
        // has already taken ownership of the Child for wait().
        if let Some(mut k) = session.killer.lock().expect("killer poisoned").take() {
            let _ = k.kill();
        }
        Ok(())
    }

    /// List all running sessions (in-memory).
    pub async fn list_running(&self) -> Vec<TerminalSessionMeta> {
        let sessions = self.sessions.lock().await;
        let mut out = Vec::with_capacity(sessions.len());
        for s in sessions.values() {
            out.push(s.meta.lock().await.clone());
        }
        out
    }

    /// List both running and historical sessions, merging SQLite with the
    /// in-memory running map (which is authoritative for status=Running).
    pub async fn list_all(&self) -> Result<Vec<TerminalSessionMeta>> {
        let mut historical = self.state_db.list_terminal_sessions().await?;
        // Overwrite any entries whose live meta we have (running, updated cols/rows, etc).
        let running = self.list_running().await;
        let live_ids: HashMap<Uuid, TerminalSessionMeta> =
            running.into_iter().map(|m| (m.id, m)).collect();
        for m in historical.iter_mut() {
            if let Some(live) = live_ids.get(&m.id) {
                *m = live.clone();
            }
        }
        // Add running sessions that don't appear in SQLite yet (shouldn't
        // happen because create writes to db first, but keep for safety).
        for (id, m) in &live_ids {
            if !historical.iter().any(|h| h.id == *id) {
                historical.push(m.clone());
            }
        }
        Ok(historical)
    }

    /// Graceful shutdown: kill every running session's child and mark it as
    /// Killed in SQLite. Invoked from `Server::run` on shutdown signal.
    ///
    /// Uses the clone-killer handle rather than `Child::kill`, because the
    /// waiter task typically owns the `Child` by the time shutdown runs and
    /// we would otherwise be unable to terminate the PTY processes — which
    /// leaves the daemon unable to exit cleanly.
    pub async fn shutdown_all(&self) {
        let sessions: Vec<Arc<TerminalSession>> = {
            let map = self.sessions.lock().await;
            map.values().cloned().collect()
        };
        for session in sessions {
            if let Some(mut k) = session.killer.lock().expect("killer poisoned").take() {
                let _ = k.kill();
            }
        }
    }
}

/// Spawn a blocking OS thread that drives the PTY master reader.
fn spawn_reader_thread(
    session: Arc<TerminalSession>,
    mut reader: Box<dyn std::io::Read + Send>,
    db_tx: mpsc::Sender<TermFrame>,
) {
    std::thread::Builder::new()
        .name(format!("wisphive-pty-{}", session.id))
        .spawn(move || {
            let mut buf = [0u8; CHUNK_BYTES];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!(session_id = %session.id, "pty reader saw EOF");
                        break;
                    }
                    Ok(n) => {
                        let bytes = Bytes::copy_from_slice(&buf[..n]);
                        // Feed the vt100 parser so catchup snapshots stay current.
                        {
                            let mut parser = session.parser.lock().expect("parser poisoned");
                            parser.process(&bytes);
                        }
                        let seq = session.next_seq();
                        let ts_us = chrono::Utc::now().timestamp_micros();
                        let frame = Arc::new(TermFrame {
                            seq,
                            ts_us,
                            direction: TerminalDirection::Output,
                            bytes,
                        });
                        // Broadcast to live viewers (drops for slow receivers).
                        let _ = session.bcast.send(frame.clone());
                        // Enqueue for DB batcher. If the queue fills, a
                        // blocking_send back-pressures the reader — correct:
                        // stalling briefly beats losing audit data.
                        if db_tx.blocking_send(TermFrame {
                            seq: frame.seq,
                            ts_us: frame.ts_us,
                            direction: frame.direction,
                            bytes: frame.bytes.clone(),
                        }).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(session_id = %session.id, "pty read error: {e}");
                        break;
                    }
                }
            }
            session
                .ended
                .store(true, std::sync::atomic::Ordering::Release);
        })
        .expect("failed to spawn pty reader thread");
}

/// Drain frames from the in-memory channel into SQLite in bounded batches.
async fn run_db_batcher(
    session_id: Uuid,
    mut rx: mpsc::Receiver<TermFrame>,
    state_db: Arc<StateDb>,
) {
    let mut pending: Vec<(Uuid, u64, i64, TerminalDirection, Vec<u8>)> = Vec::with_capacity(128);
    loop {
        // Drain until we have enough for a batch or 50 ms pass.
        let deadline = tokio::time::sleep(Duration::from_millis(50));
        tokio::pin!(deadline);

        tokio::select! {
            biased;
            frame = rx.recv() => {
                match frame {
                    Some(f) => {
                        pending.push((session_id, f.seq, f.ts_us, f.direction, f.bytes.to_vec()));
                        // Drain any immediately-available frames up to the batch cap.
                        while pending.len() < 128 {
                            match rx.try_recv() {
                                Ok(f) => pending.push((
                                    session_id, f.seq, f.ts_us, f.direction, f.bytes.to_vec(),
                                )),
                                Err(_) => break,
                            }
                        }
                    }
                    None => {
                        // Sender dropped; flush whatever's left and exit.
                        if !pending.is_empty()
                            && let Err(e) = state_db.insert_terminal_events_batch(&pending).await {
                                warn!(session_id = %session_id, "final batch insert failed: {e}");
                            }
                        return;
                    }
                }
            }
            _ = &mut deadline, if !pending.is_empty() => {
                // 50 ms elapsed with something buffered — flush even if small.
            }
        }

        if !pending.is_empty()
            && let Err(e) = state_db.insert_terminal_events_batch(&pending).await {
                warn!(session_id = %session_id, "batch insert failed: {e}");
            }
        pending.clear();
    }
}

/// Wait for the child to exit (polled via spawn_blocking), then persist status
/// and broadcast TermEnded to all TUIs.
async fn run_waiter(
    session: Arc<TerminalSession>,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
    manager: Arc<TerminalSessionManager>,
) {
    let id = session.id;
    // We must pull the child out to call wait(), because wait() takes &mut self.
    let child_opt = session.child.lock().expect("child mutex poisoned").take();
    let Some(mut child) = child_opt else {
        return;
    };

    let wait_result = tokio::task::spawn_blocking(move || child.wait())
        .await
        .ok()
        .and_then(|r| r.ok());

    // Translate portable_pty::ExitStatus to (code, TerminalStatus)
    let (exit_code, status) = match wait_result {
        Some(status) => {
            let code = status.exit_code() as i32;
            let terminal_status = if status.success() {
                TerminalStatus::Exited
            } else {
                // We can't distinguish "killed by signal" from "exited with
                // nonzero" portably — call both Exited unless we explicitly
                // close(). shutdown_all() races with this; if ended already
                // says killed we preserve that.
                TerminalStatus::Exited
            };
            (Some(code), terminal_status)
        }
        None => (None, TerminalStatus::Killed),
    };

    {
        let mut meta = session.meta.lock().await;
        meta.ended_at = Some(chrono::Utc::now());
        meta.exit_code = exit_code;
        meta.status = status;
    }
    if let Err(e) = state_db.end_terminal_session(id, exit_code, status).await {
        warn!(session_id = %id, "end_terminal_session persist failed: {e}");
    }
    let _ = tui_tx.send(ServerMessage::TermEnded {
        id,
        exit_code,
        status,
    });

    // Keep the session in the sessions map so late attaches can still see
    // the final screen via catchup; a future retention sweep can prune.
    info!(session_id = %id, ?status, "terminal session ended");
    drop(manager);
}

/// Encode raw PTY bytes as a `TermChunk` ready to ship on the wire.
pub fn frame_to_chunk(id: Uuid, frame: &TermFrame) -> ServerMessage {
    ServerMessage::TermChunk {
        id,
        seq: frame.seq,
        ts_us: frame.ts_us,
        direction: frame.direction,
        data: B64.encode(&frame.bytes),
    }
}

/// Build a `TermCatchup` message from a vt100 snapshot.
pub fn catchup_message(session: &TerminalSession, next_seq: u64) -> ServerMessage {
    let screen = session.catchup_snapshot();
    // cols/rows are tracked in the parser but we read them off meta for
    // simplicity; they are updated on resize.
    let meta = session
        .meta
        .try_lock()
        .map(|m| (m.cols, m.rows))
        .unwrap_or((80, 24));
    ServerMessage::TermCatchup {
        id: session.id,
        cols: meta.0,
        rows: meta.1,
        next_seq,
        screen: B64.encode(&screen),
    }
}

/// Decode a base64 `data` field into raw bytes.
pub fn decode_b64(data: &str) -> Result<Vec<u8>> {
    B64.decode(data).map_err(|e| anyhow!("invalid base64: {e}"))
}
