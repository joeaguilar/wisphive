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
use std::future::Future;
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
use wisphive_protocol::{ServerMessage, TerminalDirection, TerminalSessionMeta, TerminalStatus};

use crate::state::{StateDb, TerminalEnvSpec};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Test-only pause hook invoked at the top of `ingest_output` with the frame
/// bytes (itr#626 interleaving tests).
#[cfg(test)]
type IngestGapHook = Box<dyn Fn(&[u8]) + Send>;

/// Maximum PTY dimensions accepted. vt100 allocates per-cell so very large
/// terminals burn memory; wisphive rejects anything past this bound.
const MAX_COLS: u16 = 500;
const MAX_ROWS: u16 = 200;

/// Chunk size for PTY reads. Frames larger than this are split so that a
/// single client can't hold up the broadcast with a multi-megabyte write.
const CHUNK_BYTES: usize = 4096;

/// Byte cap for the scrollback seed a respawned session replays into its new
/// vt100 screen (itr#591, herdr's screen-history-replay path uses 8 KiB; we
/// afford more because the seed only ever renders as one final screen).
/// Trimmed oldest-first; see `StateDb::tail_terminal_output`.
const RESPAWN_SEED_MAX_BYTES: usize = 256 * 1024;

/// A single event from a terminal's live stream.
#[derive(Debug, Clone)]
pub struct TermFrame {
    pub seq: u64,
    pub ts_us: i64,
    pub direction: TerminalDirection,
    pub bytes: Bytes,
}

/// One running terminal session.
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
    ///
    /// itr#626: this mutex is ALSO the sequence-assignment lock. Every
    /// `next_seq()` call for a live session happens while holding it (output:
    /// `ingest_output`; input/resize: their command handlers), so under the
    /// lock the parser's content and the seq counter are always mutually
    /// consistent — which is what lets `attach_with` capture the boundary,
    /// the snapshot, and the broadcast subscription atomically.
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
    /// Graceful-shutdown preserve flag (itr#591): set by `shutdown_all` for
    /// PINNED sessions before their child is killed. The waiter then skips
    /// the end-persist, leaving the SQLite row `running` so the next
    /// daemon's startup sweep captures it as a respawn candidate ("running
    /// at shutdown" candidacy). Never set outside daemon teardown.
    preserve_status_on_shutdown: std::sync::atomic::AtomicBool,
    /// `seq + 1` of the newest OUTPUT frame ingested by the reader (0 =
    /// none this epoch). Written inside the parser/seq critical section, so
    /// an attach reading it under the same lock learns exactly which output
    /// frames its scrollback seed must cover (itr#626).
    output_high_water: AtomicU64,
    /// `seq + 1` of the newest OUTPUT frame the db batcher has successfully
    /// persisted (0 = none this epoch). Shared with `run_db_batcher`; the
    /// attach seam waits (bounded) until this reaches the high-water mark
    /// captured at the boundary so the seed read cannot miss output that has
    /// already scrolled off the screen snapshot (itr#626).
    output_persisted: Arc<AtomicU64>,
    /// Test-only pause point inside `ingest_output`, used by the itr#626
    /// race tests to hold the reader at a chosen interleaving while an
    /// attach runs. Always `None` outside tests.
    #[cfg(test)]
    ingest_gap_hook: std::sync::Mutex<Option<IngestGapHook>>,
}

impl TerminalSession {
    /// Return the next sequence number.
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel)
    }

    /// Read the sequence counter without incrementing. Only meaningful as an
    /// attach boundary when read under the parser/seq lock (see
    /// `attach_with`, itr#626); viewers use the captured value to filter out
    /// stale frames their broadcast receiver may re-deliver.
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

    #[cfg(test)]
    fn set_ingest_gap_hook(&self, hook: IngestGapHook) {
        *self.ingest_gap_hook.lock().expect("gap hook poisoned") = Some(hook);
    }

    #[cfg(test)]
    fn run_ingest_gap_hook(&self, bytes: &[u8]) {
        if let Some(hook) = self
            .ingest_gap_hook
            .lock()
            .expect("gap hook poisoned")
            .as_ref()
        {
            hook(bytes);
        }
    }

    /// Ingest one output frame from the PTY reader: update the vt100 parser,
    /// assign a sequence number, and broadcast to live viewers. Returns the
    /// frame for persistence enqueueing.
    ///
    /// itr#626: the vt100 update and the sequence assignment form ONE
    /// critical section under the parser lock. Pre-fix they were separate
    /// steps, so an attach could snapshot a parser that already contained a
    /// frame whose seq had not been assigned yet — the frame then arrived
    /// live with `seq >= next_seq` and rendered twice.
    fn ingest_output(&self, bytes: Bytes) -> Arc<TermFrame> {
        #[cfg(test)]
        self.run_ingest_gap_hook(&bytes);
        let (seq, ts_us) = {
            let mut parser = self.parser.lock().expect("parser poisoned");
            parser.process(&bytes);
            let seq = self.next_seq();
            self.output_high_water.store(seq + 1, Ordering::Release);
            (seq, chrono::Utc::now().timestamp_micros())
        };
        let frame = Arc::new(TermFrame {
            seq,
            ts_us,
            direction: TerminalDirection::Output,
            bytes,
        });
        // Broadcast to live viewers (drops for slow receivers). The send
        // happens after the critical section, so any receiver subscribed
        // under the lock (attach_with) with boundary N is guaranteed to see
        // every frame assigned seq >= N.
        let _ = self.bcast.send(frame.clone());
        frame
    }

    /// Perform the attach seam: capture the sequence boundary, read the
    /// (already ACL-decided) scrollback seed via `read_seed`, deliver the
    /// catchup via `deliver`, and return the live receiver plus the boundary
    /// the forwarder must filter against.
    ///
    /// itr#626 exactly-once invariant: the boundary `N`, the parser
    /// snapshot, and the broadcast subscription are captured in ONE critical
    /// section under the parser/seq lock, with no awaits inside. Because
    /// every seq assignment also happens under that lock:
    ///
    /// - every frame with `seq < N` already has its bytes in the snapshot
    ///   (and, for output, is covered by the seed read below); if the
    ///   receiver still sees such a frame, the forwarder's `seq < N` filter
    ///   drops it — never rendered twice;
    /// - every frame with `seq >= N` is assigned (and therefore broadcast)
    ///   strictly after the subscription exists — never dropped.
    ///
    /// The persistence wait between the critical section and the seed read
    /// closes the remaining scrollback gap: output below `N` still sitting
    /// in the db batcher would otherwise be invisible to the seed query and
    /// absent from this attachment's scrollback once it scrolls off screen.
    pub async fn attach_with<SF, DF, E>(
        &self,
        read_seed: impl FnOnce(u64) -> SF,
        deliver: impl FnOnce(ServerMessage) -> DF,
    ) -> Result<(broadcast::Receiver<Arc<TermFrame>>, u64), E>
    where
        SF: Future<Output = Vec<u8>>,
        DF: Future<Output = Result<(), E>>,
    {
        let (next_seq, snapshot, rx, settle_target) = {
            let parser = self.parser.lock().expect("parser poisoned");
            let next_seq = self.seq_load();
            let snapshot = parser.screen().contents_formatted();
            let rx = self.bcast.subscribe();
            let settle_target = self.output_high_water.load(Ordering::Acquire);
            (next_seq, snapshot, rx, settle_target)
        };
        self.await_output_persisted(settle_target, Duration::from_millis(500))
            .await;
        let seed = read_seed(next_seq).await;
        let catchup = catchup_message(self, next_seq, &snapshot, &seed);
        deliver(catchup).await?;
        Ok((rx, next_seq))
    }

    /// Bounded wait for the db batcher to persist every output frame below
    /// `target` (a high-water mark: `seq + 1`). On timeout the attach
    /// degrades loudly to whatever is persisted — the screen repaint stays
    /// authoritative for anything still on screen.
    async fn await_output_persisted(&self, target: u64, max_wait: Duration) {
        if target == 0 || self.output_persisted.load(Ordering::Acquire) >= target {
            return;
        }
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            if self.output_persisted.load(Ordering::Acquire) >= target {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                warn!(
                    session_id = %self.id,
                    target,
                    persisted = self.output_persisted.load(Ordering::Acquire),
                    "attach seed persistence catch-up timed out; seeding what is persisted"
                );
                return;
            }
        }
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
        created_by: Option<String>,
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

        let spawned =
            spawn_pty_child(id, &cmd_str, &cmd_args, &cwd_path, env.as_ref(), cols, rows)?;

        // Scrollback=0: we rely on vt100 for the current screen only; full
        // replay goes through SQLite. Keeping scrollback out of memory bounds
        // worst-case cost to O(cols*rows).
        let parser = vt100::Parser::new(rows, cols, 0);

        let (bcast_tx, _) = broadcast::channel::<Arc<TermFrame>>(256);
        let (db_tx, db_rx) = mpsc::channel::<TermFrame>(1024);

        let started_at = chrono::Utc::now();
        // Default sort_order = -started_at_ms so new sessions sort newest-first
        // before the user drags anything. User reorders overwrite this.
        let sort_order = -started_at.timestamp_millis();
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
            group_name: None,
            sort_order,
            created_by,
            replay_acl: Vec::new(),
            pinned: false,
        };
        // Persist the restorable respawn spec (itr#590): command/args/cwd
        // live on the meta row; the client-requested env overrides are
        // redaction-classified into `env_json` (see `TerminalEnvSpec` for
        // the secret-handling semantics). The daemon-inherited environment
        // is deliberately NOT captured — a reconciling daemon re-inherits
        // current values at respawn.
        let env_spec = env.as_ref().and_then(TerminalEnvSpec::from_requested_env);
        self.state_db
            .create_terminal_session(&meta, env_spec.as_ref())
            .await?;

        let session = Arc::new(TerminalSession {
            id,
            meta: Mutex::new(meta.clone()),
            writer: std::sync::Mutex::new(spawned.writer),
            master: std::sync::Mutex::new(spawned.master),
            parser: std::sync::Mutex::new(parser),
            bcast: bcast_tx.clone(),
            seq: AtomicU64::new(0),
            child: std::sync::Mutex::new(Some(spawned.child)),
            killer: std::sync::Mutex::new(Some(spawned.killer)),
            ended: std::sync::atomic::AtomicBool::new(false),
            preserve_status_on_shutdown: std::sync::atomic::AtomicBool::new(false),
            output_high_water: AtomicU64::new(0),
            output_persisted: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            ingest_gap_hook: std::sync::Mutex::new(None),
        });

        self.sessions.lock().await.insert(id, session.clone());

        // Reader thread: portable_pty's reader is blocking, so we drive it
        // from a dedicated OS thread instead of a tokio task.
        spawn_reader_thread(session.clone(), spawned.reader, db_tx.clone());

        // DB batcher: drains frames into SQLite in small transactional batches.
        tokio::spawn(run_db_batcher(
            id,
            db_rx,
            self.state_db.clone(),
            session.output_persisted.clone(),
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

        // Log the input event for faithful replay. The seq assignment takes
        // the parser lock (the sequence-assignment lock, itr#626) so that an
        // attach boundary cleanly partitions ALL frames, not just output.
        let seq = {
            let _seq_lock = session.parser.lock().expect("parser poisoned");
            session.next_seq()
        };
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
        // The screen-state change and the seq assignment share one critical
        // section (the sequence-assignment lock, itr#626): an attach snapshot
        // either sees the resized screen with the resize frame below its
        // boundary, or neither.
        let seq = {
            let mut parser = session.parser.lock().expect("parser poisoned");
            parser.set_size(rows, cols);
            session.next_seq()
        };
        {
            let mut meta = session.meta.lock().await;
            meta.cols = cols;
            meta.rows = rows;
        }

        // Log as a resize event.
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
            .insert_terminal_events_batch(&[(id, seq, ts_us, TerminalDirection::Resize, payload)])
            .await?;
        Ok(())
    }

    /// Request that portable-pty terminate a session's child process.
    ///
    /// The public API intentionally exposes one close operation rather than
    /// promising platform-specific graceful/force signal semantics.
    pub async fn close(&self, id: Uuid) -> Result<()> {
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow!("terminal session {id} not found"))?;
        // Use the clone-killer because the waiter task owns the Child handle.
        if let Some(mut k) = session.killer.lock().expect("killer poisoned").take() {
            k.kill()
                .with_context(|| format!("failed to close terminal session {id}"))?;
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
    /// in-memory running map (which is authoritative for status=Running and
    /// current PTY dimensions). Group/sort metadata always comes from the DB,
    /// so user reorders aren't clobbered by stale live meta.
    pub async fn list_all(&self) -> Result<Vec<TerminalSessionMeta>> {
        let mut historical = self.state_db.list_terminal_sessions().await?;
        let running = self.list_running().await;
        let live_ids: HashMap<Uuid, TerminalSessionMeta> =
            running.into_iter().map(|m| (m.id, m)).collect();
        for m in historical.iter_mut() {
            if let Some(live) = live_ids.get(&m.id) {
                // Keep live's status/cols/rows/ended_at/exit_code, but keep
                // DB's group_name/sort_order (the user's source of truth).
                m.status = live.status;
                m.cols = live.cols;
                m.rows = live.rows;
                m.ended_at = live.ended_at;
                m.exit_code = live.exit_code;
                m.label = live.label.clone();
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

    /// Assign a group label to a session (None clears it). Persists to SQLite
    /// and keeps the live in-memory meta in sync for running sessions.
    pub async fn set_group(&self, id: Uuid, group: Option<&str>) -> Result<()> {
        self.state_db.set_terminal_group(id, group).await?;
        let sessions = self.sessions.lock().await;
        if let Some(sess) = sessions.get(&id) {
            sess.meta.lock().await.group_name = group.map(|s| s.to_string());
        }
        Ok(())
    }

    /// Update a session's manual sort order. Persists to SQLite and mirrors
    /// to the live meta so list_all reflects the change immediately.
    pub async fn set_sort_order(&self, id: Uuid, order: i64) -> Result<()> {
        self.state_db.set_terminal_sort_order(id, order).await?;
        let sessions = self.sessions.lock().await;
        if let Some(sess) = sessions.get(&id) {
            sess.meta.lock().await.sort_order = order;
        }
        Ok(())
    }

    /// Pin or unpin a session as an "important session" (itr#589). Persists
    /// to SQLite and mirrors to the live meta, matching `set_group` /
    /// `set_sort_order`. The flag has execution consequences since itr#591:
    /// [`Self::shutdown_all`] reads it to preserve a pinned session's row as
    /// `running` through graceful shutdown, making it a respawn candidate
    /// for the next daemon's reconcile-on-start
    /// ([`Self::respawn_session`]).
    pub async fn set_pinned(&self, id: Uuid, pinned: bool) -> Result<()> {
        self.state_db.set_terminal_pinned(id, pinned).await?;
        let sessions = self.sessions.lock().await;
        if let Some(sess) = sessions.get(&id) {
            sess.meta.lock().await.pinned = pinned;
        }
        Ok(())
    }

    /// Respawn a pinned, orphaned session in place (itr#591
    /// reconcile-on-start). The session keeps its id and SQLite row: the
    /// stored respawn spec (itr#590) supplies command/args/cwd exactly and
    /// env best-effort (verbatim non-secret overrides; secret NAMES
    /// re-sourced from the daemon's CURRENT environment via
    /// `TerminalEnvSpec::materialize` — inherit-current, never
    /// replay-stale). The new epoch's vt100 screen is seeded from the
    /// recorded OUTPUT byte-history (never input bytes), a daemon-origin
    /// banner marks the restart honestly in both the screen and the audit
    /// stream, and the event seq continues past the previous epoch so
    /// `INSERT OR IGNORE` can never drop new audit bytes.
    ///
    /// Gate re-entry is by construction: the child goes through the same
    /// [`spawn_pty_child`] path as `create` — a fresh subprocess with
    /// `WISPHIVE_TERMINAL_SESSION_ID` set, whose tool calls route through
    /// `wisphive-hook` exactly like any fresh terminal. Nothing here can
    /// widen approvals or bypass the hook.
    ///
    /// Errors leave the row orphaned (and kill the child if one was already
    /// spawned) — respawn is best-effort and must never wedge daemon
    /// startup. A corrupt stored args blob is one of those errors
    /// (`get_terminal_respawn_spec` refuses to fabricate an argv), so the
    /// reconcile audits it as a FAILED respawn and the pin stays sticky.
    ///
    /// On success returns the revived meta plus best-effort degradation
    /// notes ("env degraded: …", "seed unavailable: …") for the caller's
    /// `terminal_respawn` audit detail — the respawn proceeded, but not at
    /// full fidelity, and that must never be silent.
    pub async fn respawn_session(
        self: &Arc<Self>,
        id: Uuid,
    ) -> Result<(TerminalSessionMeta, Vec<String>)> {
        if self.sessions.lock().await.contains_key(&id) {
            return Err(anyhow!("terminal session {id} is already live"));
        }
        let mut meta = self
            .state_db
            .get_terminal_session(id)
            .await?
            .ok_or_else(|| anyhow!("terminal session {id} not found"))?;
        if meta.status != TerminalStatus::Orphaned {
            return Err(anyhow!(
                "respawn requires an orphaned session (status: {})",
                meta.status
            ));
        }
        if !meta.pinned {
            return Err(anyhow!("respawn requires a pinned session"));
        }
        let spec = self
            .state_db
            .get_terminal_respawn_spec(id)
            .await?
            .ok_or_else(|| anyhow!("no respawn spec for terminal session {id}"))?;

        let mut notes: Vec<String> = Vec::new();
        if spec.env_degraded {
            warn!(
                session_id = %id,
                "respawn env overrides unreadable (malformed env_json); \
                 spawning with daemon-inherited env only"
            );
            notes.push("env degraded: malformed env_json".to_string());
        }

        // Env per the itr#590 stored semantics (chain handoff item f).
        let daemon_env: HashMap<String, String> = std::env::vars().collect();
        let env = spec.env.as_ref().map(|e| e.materialize(&daemon_env));

        let spawned = spawn_pty_child(
            id,
            &spec.command,
            &spec.args,
            &spec.cwd,
            env.as_ref(),
            meta.cols,
            meta.rows,
        )?;
        let mut killer = spawned.killer;

        // Everything DB-flavored below must not leak the fresh child on
        // failure: kill it and bail, leaving the row orphaned.
        let prepared: Result<(u64, vt100::Parser, Vec<u8>)> = async {
            // Continue the audit stream where the previous epoch stopped.
            let base_seq = self
                .state_db
                .max_terminal_event_seq(id)
                .await?
                .map_or(0, |s| s + 1);

            // Seed the new screen with the prior scrollback (output-only;
            // see tail_terminal_output for the security invariant) plus the
            // honesty banner: a respawned session is never misrepresented
            // as the same live process.
            let mut parser = vt100::Parser::new(meta.rows, meta.cols, 0);
            // Seed is best-effort: a DB read failure degrades to an
            // unseeded screen (banner only), never a failed respawn — but
            // LOUDLY (itr#591 rework: the silence was the bug).
            let tail = match self
                .state_db
                .tail_terminal_output(id, RESPAWN_SEED_MAX_BYTES, None)
                .await
            {
                Ok(tail) => tail,
                Err(e) => {
                    warn!(
                        session_id = %id,
                        "respawn scrollback seed read failed; seeding banner only: {e}"
                    );
                    notes.push(format!("seed unavailable: {e}"));
                    Vec::new()
                }
            };
            parser.process(&tail);
            let banner = respawn_banner();
            parser.process(&banner);

            // Resurrect the row BEFORE the waiter can run: if the fresh
            // child exits instantly, the waiter's end-persist must land
            // after (and override) the `running` flip, never the reverse.
            self.state_db.resurrect_terminal_session(id).await?;
            Ok((base_seq, parser, banner))
        }
        .await;
        let (base_seq, parser, banner) = match prepared {
            Ok(v) => v,
            Err(e) => {
                let _ = killer.kill();
                return Err(e);
            }
        };

        meta.status = TerminalStatus::Running;
        meta.ended_at = None;
        meta.exit_code = None;

        let (bcast_tx, _) = broadcast::channel::<Arc<TermFrame>>(256);
        let (db_tx, db_rx) = mpsc::channel::<TermFrame>(1024);

        let session = Arc::new(TerminalSession {
            id,
            meta: Mutex::new(meta.clone()),
            writer: std::sync::Mutex::new(spawned.writer),
            master: std::sync::Mutex::new(spawned.master),
            parser: std::sync::Mutex::new(parser),
            bcast: bcast_tx,
            seq: AtomicU64::new(base_seq),
            child: std::sync::Mutex::new(Some(spawned.child)),
            killer: std::sync::Mutex::new(Some(killer)),
            ended: std::sync::atomic::AtomicBool::new(false),
            preserve_status_on_shutdown: std::sync::atomic::AtomicBool::new(false),
            output_high_water: AtomicU64::new(0),
            output_persisted: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            ingest_gap_hook: std::sync::Mutex::new(None),
        });

        // Record the banner as a real (daemon-origin) output event so the
        // replayed audit history carries the restart marker too.
        let banner_seq = session.next_seq();
        let ts_us = chrono::Utc::now().timestamp_micros();
        let _ = session.bcast.send(Arc::new(TermFrame {
            seq: banner_seq,
            ts_us,
            direction: TerminalDirection::Output,
            bytes: Bytes::copy_from_slice(&banner),
        }));
        if let Err(e) = self
            .state_db
            .insert_terminal_events_batch(&[(
                id,
                banner_seq,
                ts_us,
                TerminalDirection::Output,
                banner,
            )])
            .await
        {
            warn!(session_id = %id, "respawn banner event persist failed: {e}");
        }

        self.sessions.lock().await.insert(id, session.clone());
        spawn_reader_thread(session.clone(), spawned.reader, db_tx.clone());
        tokio::spawn(run_db_batcher(
            id,
            db_rx,
            self.state_db.clone(),
            session.output_persisted.clone(),
        ));
        tokio::spawn(run_waiter(
            session.clone(),
            self.state_db.clone(),
            self.tui_tx.clone(),
            self.sessions_handle(),
        ));

        info!(
            session_id = %id,
            command = %meta.command,
            cwd = %meta.cwd.display(),
            "pinned terminal session respawned after daemon restart"
        );
        Ok((meta, notes))
    }

    /// Graceful shutdown: kill every running session's child and mark it as
    /// Killed in SQLite — except PINNED sessions, whose SQLite row keeps its
    /// `running` status (via the waiter-side preserve flag) so the next
    /// daemon's startup sweep captures them as respawn candidates (itr#591,
    /// "running at shutdown" candidacy). The pinned CHILD is still killed:
    /// Plan A never keeps a live process across a restart — the master fd
    /// dies with this daemon regardless. Invoked from `Server::run` on
    /// shutdown signal.
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
            if session.meta.lock().await.pinned {
                session
                    .preserve_status_on_shutdown
                    .store(true, Ordering::Release);
            }
            if let Some(mut k) = session.killer.lock().expect("killer poisoned").take() {
                let _ = k.kill();
            }
        }
    }
}

/// Everything the PTY layer hands back for one spawned child. Shared by
/// [`TerminalSessionManager::create`] (fresh session) and
/// [`TerminalSessionManager::respawn_session`] (itr#591 reconcile) so a
/// respawned child goes through EXACTLY the same spawn path — same
/// `WISPHIVE_TERMINAL_SESSION_ID` / `TERM` injection, same cwd handling —
/// and therefore re-enters the wisphive-hook gate like any fresh terminal
/// child. Keep every env/argv decision inside [`spawn_pty_child`]; a
/// respawn-only divergence here would be a gate-bypass hazard.
struct SpawnedPty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    reader: Box<dyn std::io::Read + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

/// Open a PTY and spawn `command` into it. See [`SpawnedPty`] for why this
/// is the single spawn path for both fresh and respawned sessions.
fn spawn_pty_child(
    id: Uuid,
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    env: Option<&HashMap<String, String>>,
    cols: u16,
    rows: u16,
) -> Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty failed")?;

    let mut builder = CommandBuilder::new(command);
    for arg in args {
        builder.arg(arg);
    }
    builder.cwd(cwd);
    builder.env("WISPHIVE_TERMINAL_SESSION_ID", id.to_string());
    builder.env("TERM", "xterm-256color");
    if let Some(extra) = env {
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

    Ok(SpawnedPty {
        master: pair.master,
        writer,
        reader,
        child,
        killer,
    })
}

/// The daemon-origin restart marker written into a respawned session's
/// screen and audit stream. Honesty affordance from the research doc: the
/// session "comes back", the PROCESS does not — say so where the user looks.
fn respawn_banner() -> Vec<u8> {
    b"\r\n\x1b[7m wisphive: daemon restarted \xe2\x80\x94 pinned session respawned; scrollback above is replayed history, the process below is a new instance \x1b[0m\r\n"
        .to_vec()
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
                        let frame = session.ingest_output(bytes);
                        // Enqueue for DB batcher. If the queue fills, a
                        // blocking_send back-pressures the reader — correct:
                        // stalling briefly beats losing audit data.
                        if db_tx
                            .blocking_send(TermFrame {
                                seq: frame.seq,
                                ts_us: frame.ts_us,
                                direction: frame.direction,
                                bytes: frame.bytes.clone(),
                            })
                            .is_err()
                        {
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
///
/// `output_persisted` is the session's persistence watermark (itr#626):
/// after each successful batch insert it advances to `seq + 1` of the newest
/// output frame in the batch, letting the attach seam wait for the seed's
/// backing rows instead of silently seeding a stale tail. Failed inserts do
/// not advance it — an attach then degrades loudly after its bounded wait.
async fn run_db_batcher(
    session_id: Uuid,
    mut rx: mpsc::Receiver<TermFrame>,
    state_db: Arc<StateDb>,
    output_persisted: Arc<AtomicU64>,
) {
    fn note_persisted(
        pending: &[(Uuid, u64, i64, TerminalDirection, Vec<u8>)],
        watermark: &AtomicU64,
    ) {
        if let Some(max) = pending
            .iter()
            .filter(|row| matches!(row.3, TerminalDirection::Output))
            .map(|row| row.1)
            .max()
        {
            watermark.fetch_max(max + 1, Ordering::AcqRel);
        }
    }
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
                        if !pending.is_empty() {
                            match state_db.insert_terminal_events_batch(&pending).await {
                                Ok(()) => note_persisted(&pending, &output_persisted),
                                Err(e) => {
                                    warn!(session_id = %session_id, "final batch insert failed: {e}");
                                }
                            }
                        }
                        return;
                    }
                }
            }
            _ = &mut deadline, if !pending.is_empty() => {
                // 50 ms elapsed with something buffered — flush even if small.
            }
        }

        if !pending.is_empty() {
            match state_db.insert_terminal_events_batch(&pending).await {
                Ok(()) => note_persisted(&pending, &output_persisted),
                Err(e) => {
                    warn!(session_id = %session_id, "batch insert failed: {e}");
                }
            }
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

    // Graceful daemon shutdown of a PINNED session (itr#591): `shutdown_all`
    // set the preserve flag before killing the child, so skip the
    // end-persist — the SQLite row stays `running` and the next startup's
    // sweep captures it as a respawn candidate. Skip the TermEnded broadcast
    // too (the daemon is going down; a "session ended" toast would misstate
    // what the restart is about to undo). If the child exited on its own
    // BEFORE shutdown set the flag, this path is not taken and the honest
    // Exited status persists — pinning never resurrects a session that died
    // by itself.
    if session
        .preserve_status_on_shutdown
        .load(std::sync::atomic::Ordering::Acquire)
    {
        manager.sessions.lock().await.remove(&id);
        info!(
            session_id = %id,
            "pinned terminal child stopped for daemon shutdown; row preserved as running for respawn"
        );
        return;
    }

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

    // Ended sessions are retained in SQLite for replay, but must no longer
    // keep their PTY handles alive in the live-session map.
    manager.sessions.lock().await.remove(&id);
    info!(session_id = %id, ?status, "terminal session ended");
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

/// Byte cap for the scrollback seed an attach catchup prepends for requesters
/// that pass the replay ACL (itr#624). Same budget as the respawn seed: the
/// tail of persisted output-direction rows, oldest trimmed first.
pub const ATTACH_SCROLLBACK_SEED_MAX_BYTES: usize = 256 * 1024;

/// Build a `TermCatchup` message from a vt100 snapshot.
///
/// `snapshot` is the screen repaint captured by `attach_with` inside its
/// boundary critical section (itr#626) — never re-snapshotted here, or the
/// repaint would drift ahead of the `next_seq` boundary. `scrollback_seed`
/// (itr#624) is prepended raw: the client resets its emulator and replays
/// these historical output bytes, which rebuilds real scrollback
/// client-side, before the authoritative screen repaint. The snapshot's
/// vt100 `contents_formatted()` prefix is `ESC[H ESC[J` (home +
/// erase-below), which never touches the emulator's scrollback, so the seam
/// cannot duplicate or wipe the seeded history. Pass an empty seed for the
/// legacy screen-only catchup (unauthorized requesters keep exactly the old
/// behavior — scrollback disclosure is replay-class and stays behind the
/// itr#98 ACL).
pub fn catchup_message(
    session: &TerminalSession,
    next_seq: u64,
    snapshot: &[u8],
    scrollback_seed: &[u8],
) -> ServerMessage {
    let mut screen = scrollback_seed.to_vec();
    screen.extend_from_slice(snapshot);
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    async fn wait_for_ready(manager: &TerminalSessionManager, id: Uuid) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(session) = manager.get(id).await
                    && session
                        .catchup_snapshot()
                        .windows(b"READY".len())
                        .any(|window| window == b"READY")
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("shell did not report ready");
    }

    async fn wait_for_persisted_exit(state_db: &StateDb, id: Uuid) -> TerminalSessionMeta {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let meta = state_db
                    .get_terminal_session(id)
                    .await
                    .expect("read terminal session")
                    .expect("terminal session exists");
                if meta.status != TerminalStatus::Running {
                    return meta;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("terminal exit was not persisted")
    }

    async fn wait_for_session_removal(manager: &TerminalSessionManager, id: Uuid) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if manager.get(id).await.is_none() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("ended terminal session was not removed from live map");
    }

    /// itr#624: a seeded catchup carries the scrollback seed BYTES first,
    /// then the authoritative screen repaint; an empty seed reproduces the
    /// legacy screen-only catchup exactly.
    #[tokio::test]
    async fn catchup_message_prepends_scrollback_seed_before_screen() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let meta = manager
            .create(
                Some("catchup-seed".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), "printf READY; read line".into()]),
                None,
                80,
                24,
                None,
                None,
            )
            .await
            .expect("create session");
        wait_for_ready(&manager, meta.id).await;
        let session = manager.get(meta.id).await.expect("session live");

        let snapshot = session.catchup_snapshot();
        let plain = catchup_message(&session, 7, &snapshot, &[]);
        let seeded = catchup_message(&session, 7, &snapshot, b"HISTORY\r\n");
        let (plain_screen, seeded_screen) = match (plain, seeded) {
            (
                ServerMessage::TermCatchup {
                    screen: p,
                    next_seq: p_seq,
                    ..
                },
                ServerMessage::TermCatchup {
                    screen: s,
                    next_seq: s_seq,
                    ..
                },
            ) => {
                assert_eq!(p_seq, 7);
                assert_eq!(s_seq, 7);
                (B64.decode(p).unwrap(), B64.decode(s).unwrap())
            }
            other => panic!("expected TermCatchup pair, got {other:?}"),
        };
        assert!(
            seeded_screen.starts_with(b"HISTORY\r\n"),
            "seed bytes must replay before the screen repaint"
        );
        assert_eq!(
            &seeded_screen[b"HISTORY\r\n".len()..],
            plain_screen.as_slice(),
            "after the seed, the seeded catchup is byte-identical to the legacy screen-only catchup"
        );
        // The repaint that follows the seed must home + erase-below — never
        // an ED2/ED3 that could disturb the client's freshly seeded
        // scrollback (vt100's ClearScreen is ESC[H ESC[J).
        assert!(
            plain_screen
                .windows(b"\x1b[H\x1b[J".len())
                .any(|w| w == b"\x1b[H\x1b[J"),
            "screen repaint should carry the home+erase-below prefix"
        );
        manager.close(meta.id).await.expect("close session");
    }

    /// Extract every `L-<n>` marker from a byte stream, in order. Markers are
    /// plain text, so this works on raw frames and on the ANSI-bearing screen
    /// repaint alike (vt100 renders a plain-text row's characters
    /// contiguously).
    fn stream_tokens(bytes: &[u8]) -> Vec<usize> {
        let text = String::from_utf8_lossy(bytes);
        let mut tokens = Vec::new();
        let mut rest = text.as_ref();
        while let Some(pos) = rest.find("L-") {
            rest = &rest[pos + 2..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                tokens.push(digits.parse::<usize>().expect("token digits"));
                rest = &rest[digits.len()..];
            }
        }
        tokens
    }

    fn count_occurrences(hay: &[u8], needle: &str) -> usize {
        String::from_utf8_lossy(hay).matches(needle).count()
    }

    fn decode_catchup_screen(msg: ServerMessage) -> Vec<u8> {
        match msg {
            ServerMessage::TermCatchup { screen, .. } => {
                B64.decode(screen).expect("catchup screen base64")
            }
            other => panic!("expected TermCatchup, got {other:?}"),
        }
    }

    /// itr#626 DROP window, deterministic: a frame produced while the attach
    /// is delivering its catchup (after the sequence boundary was captured)
    /// must still reach the attachment exactly once. Pre-fix, the frame was
    /// broadcast before the attach subscribed AND excluded from the snapshot
    /// and seed — delivered zero times.
    ///
    /// Determinism: the injection happens inside the `deliver` closure — by
    /// construction after the boundary capture and before `attach_with`
    /// returns — and the closure only returns once the injected frame is
    /// PERSISTED. The reader broadcasts before enqueueing for persistence,
    /// so a persisted marker proves the broadcast already happened.
    #[tokio::test]
    async fn frame_arriving_during_catchup_delivery_is_delivered_exactly_once() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let meta = manager
            .create(
                Some("itr626-drop".into()),
                Some("/bin/sh".into()),
                Some(vec![
                    "-c".into(),
                    "printf READY; while read l; do printf 'OUT-%s\\r\\n' \"$l\"; done".into(),
                ]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create session");
        wait_for_ready(&manager, meta.id).await;
        let session = manager.get(meta.id).await.expect("session live");

        let catchup_slot: Arc<std::sync::Mutex<Option<ServerMessage>>> =
            Arc::new(std::sync::Mutex::new(None));
        let slot = catchup_slot.clone();
        let inj_manager = manager.clone();
        let inj_db = state_db.clone();
        let sess_id = meta.id;
        let (mut rx, next_seq) = session
            .attach_with(
                |_n| async { Vec::new() },
                move |catchup| async move {
                    inj_manager
                        .write_input(sess_id, b"42\n".to_vec())
                        .await
                        .expect("inject input");
                    tokio::time::timeout(Duration::from_secs(10), async {
                        loop {
                            let tail = inj_db
                                .tail_terminal_output(sess_id, 1 << 20, None)
                                .await
                                .expect("tail read");
                            if count_occurrences(&tail, "OUT-42") > 0 {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("injected frame never persisted");
                    *slot.lock().unwrap() = Some(catchup);
                    Ok::<(), anyhow::Error>(())
                },
            )
            .await
            .expect("attach");

        // Sentinel injected AFTER attach returned: guaranteed to arrive on
        // the live stream under any implementation, so draining until it
        // shows up is deterministic.
        manager
            .write_input(meta.id, b"done\n".to_vec())
            .await
            .expect("sentinel input");

        let mut live = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = rx.recv().await.expect("broadcast closed");
                if frame.seq < next_seq {
                    // The live forwarder's filter semantics.
                    continue;
                }
                if matches!(frame.direction, TerminalDirection::Output) {
                    live.extend_from_slice(&frame.bytes);
                    if count_occurrences(&live, "OUT-done") > 0 {
                        break;
                    }
                }
            }
        })
        .await
        .expect("sentinel never arrived on the live stream");

        let catchup_screen = decode_catchup_screen(
            catchup_slot
                .lock()
                .unwrap()
                .take()
                .expect("catchup delivered"),
        );
        let total =
            count_occurrences(&catchup_screen, "OUT-42") + count_occurrences(&live, "OUT-42");
        // Close BEFORE asserting: a panicking assert would otherwise leave
        // the PTY child alive and the waiter's spawn_blocking wait pinned,
        // hanging runtime teardown.
        manager.close(meta.id).await.ok();
        assert_eq!(
            total, 1,
            "a frame produced while the attach catchup was being delivered must reach \
             the attachment exactly once (0 = dropped, itr#626 DROP; 2 = duplicated)"
        );
    }

    /// itr#626 DUPLICATE window, deterministic: the reader ingest and a full
    /// attach are interleaved at the ingest pause hook. Pre-fix the hook sat
    /// in the gap between the vt100 update and the sequence assignment, so
    /// the attach snapshot already contained the frame's bytes while the
    /// frame was then also assigned `seq >= next_seq` and delivered live —
    /// the same bytes twice. The fix collapses that gap into one critical
    /// section shared with the attach boundary capture, so the same schedule
    /// (reader paused at ingest while an attach completes) now lands the
    /// frame wholly on the live side.
    #[tokio::test]
    async fn attach_overlapping_reader_ingest_delivers_exactly_once() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let meta = manager
            .create(
                Some("itr626-dup".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), "printf READY; read line".into()]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create session");
        wait_for_ready(&manager, meta.id).await;
        let session = manager.get(meta.id).await.expect("session live");

        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        session.set_ingest_gap_hook(Box::new(move |bytes| {
            // Pause ONLY the marker frame: any straggling real PTY output
            // (e.g. the READY frame's ingest completing late) must pass
            // through untouched or it would consume the release token.
            if !bytes.windows(b"GAPMARK".len()).any(|w| w == b"GAPMARK") {
                return;
            }
            let _ = entered_tx.send(());
            // Ignore errors so the hook degrades to a no-op once the test's
            // release sender is gone.
            let _ = release_rx.lock().unwrap().recv();
        }));

        let ingest_session = session.clone();
        let ingest = std::thread::spawn(move || {
            ingest_session.ingest_output(Bytes::from_static(b"GAPMARK\r\n"))
        });
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ingest never reached the pause hook");

        let catchup_slot: Arc<std::sync::Mutex<Option<ServerMessage>>> =
            Arc::new(std::sync::Mutex::new(None));
        let slot = catchup_slot.clone();
        let (mut rx, next_seq) = session
            .attach_with(
                |_n| async { Vec::new() },
                move |catchup| async move {
                    *slot.lock().unwrap() = Some(catchup);
                    Ok::<(), anyhow::Error>(())
                },
            )
            .await
            .expect("attach");

        release_tx.send(()).expect("release ingest");
        let frame = ingest.join().expect("ingest thread");

        // The ingested frame is the only broadcast after our subscribe
        // completed (the shell is blocked on `read line`), so one bounded
        // recv drains everything relevant.
        let mut live = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while count_occurrences(&live, "GAPMARK") == 0 && tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(f)) => {
                    if f.seq >= next_seq && matches!(f.direction, TerminalDirection::Output) {
                        live.extend_from_slice(&f.bytes);
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }

        let catchup_screen = decode_catchup_screen(
            catchup_slot
                .lock()
                .unwrap()
                .take()
                .expect("catchup delivered"),
        );
        let total =
            count_occurrences(&catchup_screen, "GAPMARK") + count_occurrences(&live, "GAPMARK");
        // Close BEFORE asserting (see the DROP test for why).
        manager.close(meta.id).await.ok();
        assert_eq!(
            total, 1,
            "a frame whose ingest overlaps an attach must reach the attachment exactly \
             once (2 = duplicated via snapshot + live, itr#626 DUPLICATE; 0 = dropped); \
             ingested frame seq {} vs boundary {}",
            frame.seq, next_seq
        );
    }

    /// itr#626 acceptance: attach WHILE output is streaming and assert the
    /// attachment reconstructs EXACTLY the produced stream — every produced
    /// line exactly once, in order, across seed + screen repaint + live
    /// frames. Verified at the emulator level (a client vt100 with deep
    /// scrollback) because the catchup repaint is by design a re-rendering
    /// of bytes the seed also carries, not a byte-for-byte relay.
    #[tokio::test]
    async fn attach_mid_stream_reconstructs_exactly_the_produced_stream() {
        const LINES: usize = 200;
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let script = format!(
            "printf READY; read go; i=0; while [ \"$i\" -lt {LINES} ]; do echo \"L-$i\"; \
             i=$((i+1)); sleep 0.01; done; echo STREAM-DONE; read fin"
        );
        let meta = manager
            .create(
                Some("itr626-stream".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), script]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create session");
        wait_for_ready(&manager, meta.id).await;
        let session = manager.get(meta.id).await.expect("session live");
        manager
            .write_input(meta.id, b"go\n".to_vec())
            .await
            .expect("start stream");

        // Attach genuinely mid-stream: wait until a chunk of the burst has
        // already been produced (and partly scrolled off the 24-row screen).
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if count_occurrences(&session.catchup_snapshot(), "L-3") > 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("stream never reached mid-point");

        let catchup_slot: Arc<std::sync::Mutex<Option<ServerMessage>>> =
            Arc::new(std::sync::Mutex::new(None));
        let slot = catchup_slot.clone();
        let seed_db = state_db.clone();
        let sess_id = meta.id;
        let seed_slot: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seed_out = seed_slot.clone();
        let (mut rx, next_seq) = session
            .attach_with(
                move |n| async move {
                    let seed = seed_db
                        .tail_terminal_output(sess_id, ATTACH_SCROLLBACK_SEED_MAX_BYTES, Some(n))
                        .await
                        .expect("seed read");
                    *seed_out.lock().unwrap() = seed.clone();
                    seed
                },
                move |catchup| async move {
                    *slot.lock().unwrap() = Some(catchup);
                    Ok::<(), anyhow::Error>(())
                },
            )
            .await
            .expect("attach");

        // Collect the accepted live output stream (the forwarder's filter
        // semantics) until the terminator arrives.
        let mut live = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let frame = rx.recv().await.expect("broadcast closed");
                if frame.seq < next_seq {
                    continue;
                }
                if matches!(frame.direction, TerminalDirection::Output) {
                    live.extend_from_slice(&frame.bytes);
                    if count_occurrences(&live, "STREAM-DONE") > 0 {
                        return;
                    }
                }
            }
        })
        .await
        .expect("stream end never arrived on the live stream");

        let seed = seed_slot.lock().unwrap().clone();
        let repaint = decode_catchup_screen(
            catchup_slot
                .lock()
                .unwrap()
                .take()
                .expect("catchup delivered"),
        );
        // Close BEFORE asserting (see the DROP test for why).
        manager.close(meta.id).await.ok();

        // The attachment's byte stream is seed || live (the repaint that sits
        // between them is by design a re-RENDERING of the seed's tail — the
        // home+erase-below repaint replaces exactly the screen lines the seed
        // just drew, so it never duplicates content at the emulator level).
        // Concatenating seed and live heals a line split at the seam.
        let mut stream = seed.clone();
        stream.extend_from_slice(&live);
        let tokens = stream_tokens(&stream);
        let expected: Vec<usize> = (0..LINES).collect();
        assert_eq!(
            tokens, expected,
            "the attachment must deliver every produced line exactly once, in order, \
             across seed + live — a missing line is an itr#626 gap, a repeated line \
             is an itr#626 duplicate (boundary seq {next_seq})"
        );
        assert_eq!(
            count_occurrences(&stream, "STREAM-DONE"),
            1,
            "stream terminator must appear exactly once"
        );
        // Scrollback-gap guard: everything the repaint shows must also be in
        // the seed — a screen line absent from the seed means output below the
        // boundary was skipped by the seed read (persisted too late), i.e. the
        // "vanishes once it scrolls off" degradation of itr#626. A partially
        // rendered final line can parse as an earlier (lower) token, which the
        // subset check still covers.
        let seed_tokens = stream_tokens(&seed);
        for t in stream_tokens(&repaint) {
            assert!(
                seed_tokens.contains(&t),
                "repaint shows L-{t} but the scrollback seed does not contain it — \
                 the line would vanish from scrollback (itr#626 gap; boundary {next_seq})"
            );
        }
    }

    #[tokio::test]
    async fn manager_close_terminates_real_pty_with_single_close_behavior() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let meta = manager
            .create(
                Some("close-test".into()),
                Some("/bin/sh".into()),
                Some(vec![
                    "-c".into(),
                    "trap 'exit 0' HUP; printf READY; read line".into(),
                ]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create real PTY session");
        wait_for_ready(&manager, meta.id).await;

        manager.close(meta.id).await.expect("close terminal");

        let ended = wait_for_persisted_exit(&state_db, meta.id).await;
        assert_eq!(ended.status, TerminalStatus::Exited);
        assert_eq!(ended.exit_code, Some(0));
    }

    #[tokio::test]
    async fn ended_session_is_removed_from_live_map_and_rejects_io() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));
        let meta = manager
            .create(
                Some("exit-test".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), "exit 0".into()]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create real PTY session");

        let ended = wait_for_persisted_exit(&state_db, meta.id).await;
        assert_eq!(ended.status, TerminalStatus::Exited);
        wait_for_session_removal(&manager, meta.id).await;

        assert!(manager.get(meta.id).await.is_none());
        assert!(manager.list_running().await.is_empty());
        assert!(
            manager
                .write_input(meta.id, b"input".to_vec())
                .await
                .is_err()
        );
        assert!(manager.resize(meta.id, 100, 30).await.is_err());
    }

    async fn wait_for_snapshot_count(
        manager: &TerminalSessionManager,
        id: Uuid,
        needle: &[u8],
        want: usize,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(session) = manager.get(id).await {
                    let snap = session.catchup_snapshot();
                    let count = snap.windows(needle.len()).filter(|w| *w == needle).count();
                    if count >= want {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "screen never showed {} occurrences of {:?}",
                want,
                String::from_utf8_lossy(needle)
            )
        });
    }

    /// itr#591 AC PIN: graceful shutdown preserves a PINNED session's row as
    /// `running` (the respawn-candidacy marker) while an unpinned session is
    /// ended honestly. Both children are killed either way — Plan A never
    /// keeps a live process across a restart.
    #[tokio::test]
    async fn shutdown_all_preserves_pinned_rows_as_running() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));

        let blocker = vec!["-c".to_string(), "printf READY; read line".to_string()];
        let pinned = manager
            .create(
                Some("pinned".into()),
                Some("/bin/sh".into()),
                Some(blocker.clone()),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create pinned session");
        wait_for_ready(&manager, pinned.id).await;
        manager.set_pinned(pinned.id, true).await.unwrap();

        let unpinned = manager
            .create(
                Some("unpinned".into()),
                Some("/bin/sh".into()),
                Some(blocker),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .expect("create unpinned session");
        wait_for_ready(&manager, unpinned.id).await;

        manager.shutdown_all().await;
        wait_for_session_removal(&manager, pinned.id).await;
        wait_for_session_removal(&manager, unpinned.id).await;

        let p = state_db
            .get_terminal_session(pinned.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            TerminalStatus::Running,
            "pinned row must stay `running` at shutdown — the itr#591 candidacy marker"
        );
        assert!(p.ended_at.is_none());

        let u = state_db
            .get_terminal_session(unpinned.id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            u.status,
            TerminalStatus::Running,
            "unpinned sessions end honestly at shutdown"
        );

        // The preserved row is exactly what the next startup sweep captures.
        let candidates = state_db.mark_running_terminals_orphaned().await.unwrap();
        assert_eq!(candidates, vec![pinned.id]);
    }

    /// itr#591 AC PIN: the full resurrect path. After a (simulated) restart,
    /// `respawn_session` brings a pinned orphan back live under the SAME id:
    /// saved cwd/command, prior scrollback seeded into the new screen, the
    /// honesty banner present, the audit seq stream continued without
    /// clobbering the old epoch, and `WISPHIVE_TERMINAL_SESSION_ID`
    /// re-injected into the fresh child (the hook's session cross-reference —
    /// the respawned child re-enters the gate like any fresh terminal).
    #[tokio::test]
    async fn respawn_session_revives_pinned_orphan_with_seeded_scrollback() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(
            state_db.clone(),
            tui_tx.clone(),
        ));

        let cwd = tempfile::tempdir().expect("tempdir");
        let cwd_path = cwd.path().canonicalize().expect("canonicalize cwd");
        // Prints a marker + its own session id, then blocks (stays running).
        let script = "printf 'MARKER-ALPHA sid=%s cwd=%s READY' \"$WISPHIVE_TERMINAL_SESSION_ID\" \"$(pwd -P)\"; read line";

        let meta = manager
            .create(
                Some("pinned-src".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), script.into()]),
                Some(cwd_path.clone()),
                200,
                50,
                None,
                Some("test".into()),
            )
            .await
            .expect("create session");
        let id = meta.id;
        wait_for_ready(&manager, id).await;
        manager.set_pinned(id, true).await.unwrap();

        // Wait until the marker output is PERSISTED (the respawn seed reads
        // the DB, not the live parser).
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let tail = state_db
                    .tail_terminal_output(id, 65536, None)
                    .await
                    .unwrap();
                if tail.windows(5).any(|w| w == b"READY") {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("marker output was not persisted");
        let old_max_seq = state_db
            .max_terminal_event_seq(id)
            .await
            .unwrap()
            .expect("events recorded");
        let old_epoch_rows = state_db.replay_terminal_events(id, None).await.unwrap();

        // Simulated graceful restart: shutdown kills the child but preserves
        // the pinned row as `running` (also reaps the child's blocking
        // `wait()` — a child left alive would wedge the test runtime's
        // drop). Then the next daemon's StateDb::open sweep runs; a fresh
        // manager models the fresh daemon process.
        manager.shutdown_all().await;
        wait_for_session_removal(&manager, id).await;
        let candidates = state_db.mark_running_terminals_orphaned().await.unwrap();
        assert_eq!(candidates, vec![id]);
        let manager2 = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));

        let (revived, notes) = manager2
            .respawn_session(id)
            .await
            .expect("respawn pinned orphan");
        assert_eq!(revived.id, id, "same logical session, same id");
        assert_eq!(revived.status, TerminalStatus::Running);
        assert!(revived.pinned, "pin is sticky across the respawn");
        assert!(
            notes.is_empty(),
            "a healthy respawn carries no degradation notes: {notes:?}"
        );

        // DB row resurrected.
        let row = state_db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(row.status, TerminalStatus::Running);
        assert!(row.ended_at.is_none());

        // Live + attachable in the new manager.
        let session = manager2.get(id).await.expect("respawned session live");

        // Screen: seeded old scrollback AND the fresh child's new print —
        // the marker appears twice, the banner once, and the fresh child
        // echoes the SAME session id (env re-injection) and the SAME cwd.
        wait_for_snapshot_count(&manager2, id, b"MARKER-ALPHA", 2).await;
        let screen = String::from_utf8_lossy(&session.catchup_snapshot()).to_string();
        assert!(
            screen.contains("pinned session respawned"),
            "honesty banner missing from the seeded screen: {screen}"
        );
        let sid_needle = format!("sid={id}");
        assert_eq!(
            screen.matches(&sid_needle).count(),
            2,
            "fresh child must carry WISPHIVE_TERMINAL_SESSION_ID={id}: {screen}"
        );
        let cwd_needle = format!("cwd={}", cwd_path.display());
        assert_eq!(
            screen.matches(&cwd_needle).count(),
            2,
            "respawn landed in the wrong cwd: {screen}"
        );

        // Audit stream: old epoch untouched, new epoch strictly after it.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let max = state_db.max_terminal_event_seq(id).await.unwrap().unwrap();
                if max > old_max_seq {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("new epoch events were not recorded");
        let all_rows = state_db.replay_terminal_events(id, None).await.unwrap();
        assert_eq!(
            &all_rows[..old_epoch_rows.len()],
            &old_epoch_rows[..],
            "respawn must never rewrite or drop the previous epoch's audit rows"
        );
        let banner_row = all_rows
            .iter()
            .find(|(seq, ..)| *seq == old_max_seq + 1)
            .expect("banner recorded at the epoch boundary");
        assert!(
            String::from_utf8_lossy(&banner_row.3).contains("respawned"),
            "epoch boundary event must be the daemon-origin banner"
        );

        manager2.close(id).await.expect("close respawned");
    }

    /// itr#591 candidacy negatives: respawn refuses sessions that are live,
    /// unpinned, or ended on their own — only a pinned orphan qualifies.
    #[tokio::test]
    async fn respawn_session_rejects_non_candidates() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));

        // Live session (still running in this manager): refused.
        let live = manager
            .create(
                Some("live".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), "printf READY; read line".into()]),
                None,
                80,
                24,
                None,
                Some("test".into()),
            )
            .await
            .unwrap();
        wait_for_ready(&manager, live.id).await;
        manager.set_pinned(live.id, true).await.unwrap();
        let err = manager.respawn_session(live.id).await.unwrap_err();
        assert!(err.to_string().contains("already live"), "{err}");

        // Unpinned orphan: refused.
        let unpinned = uuid::Uuid::new_v4();
        let mut m = TerminalSessionMeta {
            id: unpinned,
            label: None,
            command: "/bin/sh".into(),
            args: vec![],
            cwd: std::path::PathBuf::from("/tmp"),
            cols: 80,
            rows: 24,
            started_at: chrono::Utc::now(),
            ended_at: None,
            exit_code: None,
            status: TerminalStatus::Orphaned,
            group_name: None,
            sort_order: 0,
            created_by: None,
            replay_acl: Vec::new(),
            pinned: false,
        };
        state_db.create_terminal_session(&m, None).await.unwrap();
        let err = manager.respawn_session(unpinned).await.unwrap_err();
        assert!(err.to_string().contains("pinned"), "{err}");

        // Pinned but exited on its own: refused (pin marks importance, it
        // does not resurrect the dead).
        let exited = uuid::Uuid::new_v4();
        m.id = exited;
        m.status = TerminalStatus::Exited;
        m.pinned = true;
        state_db.create_terminal_session(&m, None).await.unwrap();
        let err = manager.respawn_session(exited).await.unwrap_err();
        assert!(err.to_string().contains("orphaned"), "{err}");

        manager.close(live.id).await.unwrap();
    }

    /// itr#590 AC#3 PIN: a respawn driven purely from the stored spec lands
    /// in the same cwd with the same command; non-secret env is reproduced
    /// verbatim; secret env is re-sourced from the (simulated) current daemon
    /// environment — never replayed stale, never persisted cleartext.
    #[tokio::test]
    async fn respawn_from_stored_spec_reproduces_cwd_command_and_env() {
        let state_db = Arc::new(StateDb::open(":memory:").await.expect("open test db"));
        let (tui_tx, _) = broadcast::channel(16);
        let manager = Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx));

        let cwd = tempfile::tempdir().expect("tempdir");
        let cwd_path = cwd.path().canonicalize().expect("canonicalize cwd");
        let script = "printf 'CWD=%s M=%s K=%s READY' \"$(pwd -P)\" \"$MY_MARKER\" \"$MY_API_KEY\"; read line";
        let env: HashMap<String, String> = [
            ("MY_MARKER".to_string(), "mv1".to_string()),
            ("MY_API_KEY".to_string(), "sk-stale12345678".to_string()),
        ]
        .into();

        let original = manager
            .create(
                Some("respawn-src".into()),
                Some("/bin/sh".into()),
                Some(vec!["-c".into(), script.into()]),
                Some(cwd_path.clone()),
                200,
                50,
                Some(env),
                Some("test".into()),
            )
            .await
            .expect("create original session");
        wait_for_ready(&manager, original.id).await;
        manager.close(original.id).await.expect("close original");
        wait_for_persisted_exit(&state_db, original.id).await;

        // Read back the stored spec — this is all itr#591's reconciler will
        // have after a daemon restart.
        let spec = state_db
            .get_terminal_respawn_spec(original.id)
            .await
            .expect("read respawn spec")
            .expect("respawn spec exists");
        assert_eq!(spec.command, "/bin/sh");
        assert_eq!(spec.cwd, cwd_path);
        let env_spec = spec.env.expect("env spec persisted");
        assert!(
            env_spec.set.values().all(|v| !v.contains("sk-stale")),
            "secret value persisted cleartext in the stored spec"
        );
        assert!(env_spec.inherit_names.contains(&"MY_API_KEY".to_string()));

        // Materialize against a simulated *current* daemon environment that
        // carries a rotated secret: inherit-current, not replay-stale.
        let daemon_env: HashMap<String, String> =
            [("MY_API_KEY".to_string(), "sk-fresh87654321".to_string())].into();
        let respawn_env = env_spec.materialize(&daemon_env);

        let respawned = manager
            .create(
                Some("respawn-dst".into()),
                Some(spec.command),
                Some(spec.args),
                Some(spec.cwd),
                200,
                50,
                Some(respawn_env),
                Some("test".into()),
            )
            .await
            .expect("respawn from stored spec");
        wait_for_ready(&manager, respawned.id).await;

        let session = manager.get(respawned.id).await.expect("respawned live");
        let screen = String::from_utf8_lossy(&session.catchup_snapshot()).to_string();
        let expected_cwd = format!("CWD={}", cwd_path.display());
        assert!(
            screen.contains(&expected_cwd),
            "respawn landed in the wrong cwd: {screen}"
        );
        assert!(
            screen.contains("M=mv1"),
            "non-secret env not reproduced verbatim: {screen}"
        );
        assert!(
            screen.contains("K=sk-fresh87654321"),
            "secret env not re-sourced from the daemon environment: {screen}"
        );
        assert!(
            !screen.contains("sk-stale"),
            "stale secret was replayed into the respawn: {screen}"
        );

        manager.close(respawned.id).await.expect("close respawned");
    }
}
