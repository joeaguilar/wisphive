use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, error, info, warn};
use wisphive_protocol::{
    ClientMessage, ClientType, Decision, PROTOCOL_VERSION, RichDecision, ServerMessage, encode,
};

use crate::config::DaemonConfig;

/// Capacity of the per-TUI-connection worker channel (itr#82). Bounds memory
/// when a fast producer (terminal forwarder, `TermReplay`) outruns a slow TUI
/// socket: at most this many `ServerMessage`s queue before producers start
/// dropping. Large enough to absorb a healthy burst between select wake-ups,
/// small enough to cap worst-case RAM.
const CONN_CHANNEL_CAPACITY: usize = 1024;

/// Maximum bytes a single newline-delimited line may occupy on a daemon socket
/// reader before the connection is rejected (itr#83). Without a cap a peer that
/// streams bytes with no newline grows the line buffer until OOM. Aligned with
/// the hook's 8 MiB stdin cap — comfortably above the largest legitimate
/// message (queue snapshots, terminal catch-up) yet far below a memory threat.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
use crate::process_registry::ProcessRegistry;
use crate::queue::DecisionQueue;
use crate::registry::AgentRegistry;
use crate::state::StateDb;
use crate::sudo_gate::ReauthRegistry;
use crate::terminal::TerminalSessionManager;

/// Shared context passed to each connection handler, replacing many individual arguments.
struct ConnectionContext {
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
    terminal_manager: Arc<TerminalSessionManager>,
    reauth: ReauthRegistry,
    hook_timeout_secs: u64,
    notifications_enabled: bool,
    home_dir: PathBuf,
}

/// The main daemon server. Listens on a Unix socket and dispatches
/// hook and TUI connections.
pub struct Server {
    config: DaemonConfig,
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
    terminal_manager: Arc<TerminalSessionManager>,
    reauth: ReauthRegistry,
}

impl Server {
    pub async fn new(config: DaemonConfig) -> Result<Self> {
        config.ensure_dirs()?;

        let (tui_tx, _) = broadcast::channel(256);
        let queue = Arc::new(Mutex::new(DecisionQueue::new(tui_tx.clone())));

        let db_path = config.db_path.to_string_lossy().to_string();
        let state_db = Arc::new(StateDb::open(&db_path).await?);

        // Crash recovery (itr#299): any pending_decisions row present now is an
        // in-flight decision from a prior process. Its hook already fail-open-
        // approved when this socket last died (DaemonUnreachable, ADR-0001), so
        // there is nothing to re-queue — record the truthful outcome and clear
        // the table before accepting new work. Runs in `new()`, before `run()`
        // binds the socket, so no live hook can race the drain.
        match state_db.drain_orphaned_pending().await {
            Ok(0) => {}
            Ok(n) => info!(drained = n, "recovered orphaned pending decisions from prior run"),
            Err(e) => warn!("failed to drain orphaned pending decisions: {e}"),
        }
        let process_registry = Arc::new(Mutex::new(ProcessRegistry::new()));
        let agent_registry = Arc::new(Mutex::new(AgentRegistry::new()));
        let terminal_manager = Arc::new(TerminalSessionManager::new(
            state_db.clone(),
            tui_tx.clone(),
        ));

        Ok(Self {
            config,
            queue,
            process_registry,
            agent_registry,
            tui_tx,
            state_db,
            terminal_manager,
            reauth: ReauthRegistry::new(),
        })
    }

    /// Returns a clone of the reauth registry. Handed to the web layer so
    /// its `/api/auth/reauth` handler can refresh devices' sudo freshness
    /// directly when it's embedded in the same process as the daemon.
    /// Standalone web binaries that connect over the socket use the
    /// `MarkDeviceFresh` command instead; both code paths end up calling
    /// [`ReauthRegistry::touch`].
    pub fn reauth_registry(&self) -> ReauthRegistry {
        self.reauth.clone()
    }

    /// Probe the audit archive size and free disk, and broadcast any
    /// raise/clear transition to TUI/web clients (also logged in `disk_alert`).
    /// Wisphive never deletes audit data; this is the non-destructive signal
    /// that prompts the operator to act (itr#340). `state` latches so each
    /// condition alerts once per crossing rather than every tick.
    fn check_disk_alerts(&self, state: &mut crate::disk_alert::AlertState) {
        let thresholds = crate::disk_alert::Thresholds {
            archive_max_bytes: self.config.archive_alert_max_bytes,
            disk_free_min_bytes: self.config.disk_alert_free_bytes,
        };
        let events = crate::disk_alert::check(
            &self.config.log_dir,
            &self.config.home_dir,
            thresholds,
            state,
        );
        for ev in events {
            let _ = self.tui_tx.send(ServerMessage::DiskAlert {
                kind: ev.kind,
                active: ev.active,
                message: ev.message,
                at: chrono::Utc::now(),
            });
        }
    }

    /// Start listening for connections. Runs until shutdown signal.
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
        // Clean up stale runtime state from a prior daemon process
        let _ = std::fs::remove_file(&self.config.socket_path);
        sweep_stale_session_markers(&self.config.home_dir.join("sessions"));

        let listener = UnixListener::bind(&self.config.socket_path)?;
        // Lock the socket down to owner-only (0600) immediately after bind,
        // before any client can connect. `bind` honours the process umask, so
        // a permissive umask could otherwise leave the socket group/world
        // accessible; this fchmod-equivalent forces owner-only regardless.
        // Defence-in-depth alongside the per-accept peer-credential check below
        // and the 0700 home dir set in `DaemonConfig::ensure_dirs` (itr#81).
        set_socket_permissions(&self.config.socket_path)?;
        info!(path = %self.config.socket_path.display(), "listening");

        // Spawn event ingest task (tails events.jsonl → decision_log)
        let events_path = self.config.home_dir.join("events.jsonl");
        let _ingest_handle = crate::event_ingest::spawn_event_ingest(
            events_path,
            self.config.log_dir.clone(),
            self.state_db.clone(),
        );

        let mut reap_interval = tokio::time::interval(Duration::from_secs(5));
        reap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Single-flight guard: the startup pass and the hourly tick must never
        // run concurrently (two overlapping VACUUM/archive passes contend on the
        // pool and can double-archive). `try_lock` → skip if a pass is running.
        let retention_lock = Arc::new(tokio::sync::Mutex::new(()));

        // Run retention on startup, but OFF the accept path: a VACUUM (or a slow
        // archive) must never delay the daemon from accepting connections, and a
        // hang/OOM there previously bricked startup entirely.
        let archive_path = self.config.log_dir.join("decision_log.jsonl");
        {
            let state_db = self.state_db.clone();
            let archive_path = archive_path.clone();
            let max_rows = self.config.retention_max_rows;
            let max_age_days = self.config.retention_max_age_days;
            let vacuum_max_bytes = self.config.retention_vacuum_max_bytes;
            let retention_lock = retention_lock.clone();
            tokio::spawn(async move {
                let _guard = match retention_lock.try_lock() {
                    Ok(g) => g,
                    Err(_) => return, // a tick already started a pass
                };
                match state_db
                    .run_retention(&archive_path, max_rows, max_age_days, vacuum_max_bytes)
                    .await
                {
                    Ok(o) if o.is_noop() => {}
                    Ok(o) => info!(
                        archived = o.archived,
                        terminal_events_pruned = o.terminal_events_pruned,
                        vacuumed = o.vacuumed,
                        "startup retention"
                    ),
                    Err(e) => warn!("startup retention failed: {e}"),
                }
            });
        }

        let mut retention_interval = tokio::time::interval(Duration::from_secs(3600));
        retention_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        retention_interval.tick().await; // skip immediate tick (we just ran on startup)

        // Latched resource-alert state (archive size, low disk). Probed inline —
        // a dir scan + statvfs is cheap, unlike the retention VACUUM that runs
        // off the accept path. Run once at startup, then on each retention tick.
        let mut alert_state = crate::disk_alert::AlertState::default();
        self.check_disk_alerts(&mut alert_state);

        loop {
            tokio::select! {
                // Periodically reap exited agent processes and inactive agents
                _ = reap_interval.tick() => {
                    let mut pr = self.process_registry.lock().await;
                    let exited = pr.reap_exited().await;
                    for (agent_id, exit_code) in exited {
                        let _ = self.tui_tx.send(ServerMessage::AgentExited {
                            agent_id,
                            exit_code,
                        });
                    }
                    drop(pr);

                    // Reap agents inactive beyond timeout
                    let agent_timeout = Duration::from_secs(self.config.agent_timeout_secs);
                    let mut reg = self.agent_registry.lock().await;
                    let reaped = reg.reap_inactive(agent_timeout);
                    drop(reg);
                    for agent_id in reaped {
                        // Clean up session marker file
                        let marker = self.config.home_dir.join("sessions").join(&agent_id);
                        let _ = std::fs::remove_file(marker);
                        let _ = self.tui_tx.send(ServerMessage::AgentDisconnected { agent_id });
                    }
                }
                // Periodic retention: archive decision_log + prune terminal_events,
                // checkpoint the WAL, and size-guarded VACUUM. Skipped if the
                // startup pass (or a prior tick) is still running.
                _ = retention_interval.tick() => {
                    match retention_lock.try_lock() {
                        Ok(_guard) => match self.state_db.run_retention(
                            &archive_path,
                            self.config.retention_max_rows,
                            self.config.retention_max_age_days,
                            self.config.retention_vacuum_max_bytes,
                        ).await {
                            Ok(o) if o.is_noop() => {}
                            Ok(o) => info!(
                                archived = o.archived,
                                terminal_events_pruned = o.terminal_events_pruned,
                                vacuumed = o.vacuumed,
                                "retention"
                            ),
                            Err(e) => warn!("retention failed: {e}"),
                        },
                        Err(_) => debug!("retention already in progress; skipping tick"),
                    }
                    // Independent of whether retention ran, re-probe resources:
                    // the archive only grows (it is never reaped) so its alert
                    // must be evaluated even when a tick is skipped.
                    self.check_disk_alerts(&mut alert_state);
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _addr)) => {
                            // Peer-credential gate: only the daemon's own uid may
                            // drive the control plane. A different local uid that
                            // can reach the socket (e.g. via a permissive parent
                            // dir or a shared host) is logged and dropped, never
                            // crashing the accept loop. The 0600 socket perms make
                            // this rare; the check is the second layer (itr#81).
                            let daemon_uid = current_uid();
                            match stream.peer_cred() {
                                Ok(cred) => {
                                    if !peer_uid_allowed(cred.uid(), daemon_uid) {
                                        warn!(
                                            peer_uid = cred.uid(),
                                            daemon_uid,
                                            "rejecting connection from foreign uid"
                                        );
                                        // Drop the stream — the connection is closed.
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    // Can't establish the peer's identity; fail
                                    // closed and drop rather than serve an
                                    // unauthenticated peer.
                                    warn!("rejecting connection: peer_cred lookup failed: {e}");
                                    continue;
                                }
                            }
                            let ctx = Arc::new(ConnectionContext {
                                queue: self.queue.clone(),
                                process_registry: self.process_registry.clone(),
                                agent_registry: self.agent_registry.clone(),
                                tui_tx: self.tui_tx.clone(),
                                state_db: self.state_db.clone(),
                                terminal_manager: self.terminal_manager.clone(),
                                reauth: self.reauth.clone(),
                                hook_timeout_secs: self.config.hook_timeout_secs,
                                notifications_enabled: self.config.notifications_enabled,
                                home_dir: self.config.home_dir.clone(),
                            });
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, &ctx).await {
                                    warn!("connection error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("shutdown signal received");
                    break;
                }
            }
        }

        // Shutdown managed agent processes
        {
            let mut pr = self.process_registry.lock().await;
            if !pr.is_empty() {
                info!("stopping {} managed agent(s)", pr.len());
                pr.shutdown_all().await;
            }
        }

        // Shutdown terminal sessions (kill PTY children).
        self.terminal_manager.shutdown_all().await;

        // Cleanup socket
        let _ = std::fs::remove_file(&self.config.socket_path);
        info!("server stopped");
        Ok(())
    }
}

/// Reader half of a daemon socket connection. A plain [`BufReader`] (not
/// [`tokio::io::Lines`]) so every read goes through [`read_capped_line`], which
/// enforces [`MAX_LINE_BYTES`] (itr#83).
type SocketReader = BufReader<tokio::net::unix::OwnedReadHalf>;

/// Read one newline-delimited line into `buf`, capping it at [`MAX_LINE_BYTES`]
/// (itr#83).
///
/// Mirrors [`tokio::io::Lines::next_line`] semantics: returns `Ok(None)` at
/// clean EOF, `Ok(Some(line))` with the trailing `\n`/`\r\n` stripped. Unlike
/// `next_line`/`read_until`, it bounds memory: it pulls from the buffered reader
/// in chunks and bails with an error the moment the accumulated line would
/// exceed the cap, so a peer streaming bytes with no newline can never grow the
/// buffer past `MAX_LINE_BYTES` and OOM the daemon.
///
/// `buf` is the caller-owned partial-line accumulator. It is passed in (rather
/// than allocated here) so this future stays cancel-safe inside `tokio::select!`:
/// if a sibling branch fires and drops this future after some bytes have been
/// consumed from the reader, the partial line survives in `buf` and the next
/// call resumes it instead of corrupting the framing. On a successful return the
/// helper clears `buf` for the next line.
async fn read_capped_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<Option<String>> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF. A trailing partial line (no newline) is still returned, same
            // as `read_until`; a truly empty buffer is a clean close.
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(idx) => {
                if buf.len() + idx > MAX_LINE_BYTES {
                    return Err(anyhow::anyhow!("line exceeded {MAX_LINE_BYTES}-byte cap"));
                }
                buf.extend_from_slice(&available[..idx]);
                reader.consume(idx + 1); // drop the consumed bytes incl. '\n'
                break;
            }
            None => {
                let take = available.len();
                if buf.len() + take > MAX_LINE_BYTES {
                    // Even without a newline yet, the line is already too long.
                    return Err(anyhow::anyhow!("line exceeded {MAX_LINE_BYTES}-byte cap"));
                }
                buf.extend_from_slice(available);
                reader.consume(take);
            }
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    let line = String::from_utf8(std::mem::take(buf))?;
    Ok(Some(line))
}

/// Handle a single client connection. Dispatches based on the Hello handshake.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(stream: UnixStream, ctx: &ConnectionContext) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader);
    let mut line_buf = Vec::new();

    // Read the Hello handshake
    let first_line = read_capped_line(&mut lines, &mut line_buf)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before hello"))?;

    let hello: ClientMessage = wisphive_protocol::decode(&first_line)?;

    match hello {
        ClientMessage::Hello { client, version } => {
            if version != PROTOCOL_VERSION {
                write_msg(
                    &mut writer,
                    &ServerMessage::Error {
                        message: format!("unsupported protocol version: {version}"),
                    },
                )
                .await?;
                return Ok(());
            }

            write_msg(
                &mut writer,
                &ServerMessage::Welcome {
                    version: PROTOCOL_VERSION,
                },
            )
            .await?;

            match client {
                ClientType::Hook => handle_hook(lines, writer, ctx).await,
                ClientType::Tui => handle_tui(lines, writer, ctx).await,
            }
        }
        _ => {
            write_msg(
                &mut writer,
                &ServerMessage::Error {
                    message: "expected Hello as first message".into(),
                },
            )
            .await?;
            Ok(())
        }
    }
}

/// Handle a hook connection: receive DecisionRequest, block until resolved.
#[allow(clippy::too_many_arguments)]
async fn handle_hook(
    mut lines: SocketReader,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
) -> Result<()> {
    let mut line_buf = Vec::new();
    let line = read_capped_line(&mut lines, &mut line_buf)
        .await?
        .ok_or_else(|| anyhow::anyhow!("hook disconnected before sending request"))?;

    let msg: ClientMessage = wisphive_protocol::decode(&line)?;

    match msg {
        ClientMessage::DecisionRequest(req) => {
            let id = req.id;
            let agent_id = req.agent_id.clone();
            let req_tool_name = req.tool_name.clone();
            // The daemon's configured state root, not a fresh $HOME lookup —
            // they diverge when the daemon runs with a non-default home (itr#360).
            let config_home = ctx.home_dir.clone();

            // Register agent and broadcast to TUI clients (only if new)
            let (agent_info, is_new) = {
                let mut reg = ctx.agent_registry.lock().await;
                reg.register(
                    agent_id.clone(),
                    req.agent_type.clone(),
                    req.project.clone(),
                )
            };
            if is_new {
                let _ = ctx.tui_tx.send(ServerMessage::AgentConnected(agent_info));
            }

            // Persist for crash recovery. INSERT OR IGNORE (itr#370): even if
            // an id collides, the first request's row is never overwritten.
            ctx.state_db.persist_pending(&req).await?;

            // Send passive notification so user knows to check the TUI
            if ctx.notifications_enabled {
                crate::notify::notify_decision(&req);
            }

            // Enqueue and get receiver. A duplicate id is rejected fail-closed
            // (itr#370): the id is hook-supplied, and overwriting would drop
            // the first request's oneshot — an instant fail-open approve for
            // the victim — and corrupt its audit row.
            let rx = {
                let mut q = ctx.queue.lock().await;
                q.enqueue(req)
            };
            let Some(rx) = rx else {
                write_msg(
                    &mut writer,
                    &ServerMessage::DecisionResponse {
                        id,
                        decision: Decision::Deny,
                        message: Some(
                            "Wisphive rejected this request: a decision with the same id is already pending"
                                .into(),
                        ),
                        updated_input: None,
                        additional_context: None,
                        selected_permission: None,
                    },
                )
                .await?;
                return Ok(());
            };

            // Block until a human responds, the timeout fires, or the hook's
            // socket closes. `decided_by` records the resolving actor for the
            // audit trail (itr#397) — the fallback resolutions are non-human
            // decisions and must be attributed as such. Watching the read half
            // (itr#363) means a dead hook (Ctrl-C'd agent) releases its queue
            // slot immediately instead of holding it for the full timeout.
            let timeout_secs = ctx.hook_timeout_secs;
            enum Waited {
                Resolved(Box<RichDecision>),
                ChannelDropped,
                TimedOut,
                Disconnected,
            }
            let waited = {
                let mut disconnect_buf = Vec::new();
                tokio::select! {
                    res = rx => match res {
                        Ok(rich) => Waited::Resolved(Box::new(rich)),
                        Err(_) => Waited::ChannelDropped,
                    },
                    () = tokio::time::sleep(Duration::from_secs(timeout_secs)) => Waited::TimedOut,
                    // Hooks send nothing after the request, so ANY completion
                    // here (EOF, error, or unexpected bytes) means the hook is
                    // gone or misbehaving — the decision is moot.
                    _ = read_capped_line(&mut lines, &mut disconnect_buf) => Waited::Disconnected,
                }
            };

            let (rich, decided_by) = match waited {
                Waited::Resolved(rich) => {
                    // Attribute the resolution to the identified client
                    // (itr#88); plain "human" only when identity didn't travel.
                    let label = rich.resolver.clone().unwrap_or_else(|| "human".to_string());
                    (*rich, label)
                }
                Waited::ChannelDropped => {
                    // Intentional fail-open (itr#345): with duplicate ids
                    // rejected (itr#370) and resolve/finalize always sending
                    // or consuming the sender, the only way the sender drops
                    // unsent is queue teardown — the daemon shutting down
                    // mid-wait. That is the daemon-down case, which fails
                    // open per ADR-0001 so a control-plane restart can't
                    // brick every waiting agent. The approve is attributed
                    // ("channel_dropped:approve") so it stays auditable.
                    warn!(%id, "decision channel dropped, defaulting to approve");
                    // Remove the leaked queue entry so TUI/web state matches
                    // the audit log (itr#363).
                    let mut q = ctx.queue.lock().await;
                    q.finalize_local(id, Decision::Approve);
                    (
                        RichDecision::approve(),
                        "channel_dropped:approve".to_string(),
                    )
                }
                Waited::TimedOut => {
                    warn!(%id, "hook timed out after {timeout_secs}s, defaulting to approve");
                    // Same cleanup: without it the item stays pending forever
                    // and a later human Deny would contradict the audit log
                    // while the tool already ran (itr#363).
                    let mut q = ctx.queue.lock().await;
                    q.finalize_local(id, Decision::Approve);
                    (RichDecision::approve(), "timeout:approve".to_string())
                }
                Waited::Disconnected => {
                    warn!(%id, "hook disconnected while awaiting decision; abandoning");
                    {
                        let mut q = ctx.queue.lock().await;
                        q.finalize_local(id, Decision::Deny);
                    }
                    // The tool did NOT run — the hook died before receiving an
                    // answer. Record a deny so the audit stream is complete.
                    ctx.state_db
                        .resolve_pending_by(id, Decision::Deny, "hook_disconnected:abandoned")
                        .await?;
                    return Ok(());
                }
            };

            // Persist auto-approve if requested (blocking file I/O off the runtime)
            if rich.always_allow {
                let tool = req_tool_name.clone();
                let persisted =
                    tokio::task::spawn_blocking(move || persist_auto_approve(&tool, &config_home))
                        .await;
                match persisted {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("failed to persist auto-approve: {e}"),
                    Err(e) => warn!("persist auto-approve task panicked: {e}"),
                }
            }

            // Log resolution. An Ask/defer is not an auditable terminal
            // decision, so it skips decision_log — but the pending row must
            // still be removed (itr#298), or it leaks: retention never reaps
            // pending_decisions, and the startup drain (itr#299) would later
            // mis-record it as a crash orphan.
            if rich.decision == Decision::Ask {
                ctx.state_db.delete_pending(id).await?;
            } else {
                ctx.state_db
                    .resolve_pending_by(id, rich.decision, &decided_by)
                    .await?;
            }

            // Touch last_seen (agent stays registered, reaped on inactivity)
            {
                let mut reg = ctx.agent_registry.lock().await;
                reg.touch(&agent_id);
            }

            // Send rich response to hook
            write_msg(
                &mut writer,
                &ServerMessage::DecisionResponse {
                    id,
                    decision: rich.decision,
                    message: rich.message,
                    updated_input: rich.updated_input,
                    additional_context: rich.additional_context,
                    selected_permission: rich.selected_permission,
                },
            )
            .await?;
        }
        ClientMessage::ToolResult(result) => {
            // Touch last_seen for the agent
            {
                let mut reg = ctx.agent_registry.lock().await;
                reg.touch(&result.agent_id);
            }
            // Fire-and-forget: attach result to matching decision_log entry.
            // Detached task because the retry schedule (itr#302) outlives this
            // ephemeral hook connection.
            let state_db = ctx.state_db.clone();
            tokio::spawn(async move {
                match attach_tool_result_with_retry(&state_db, &result, ATTACH_RETRY_DELAYS).await {
                    Ok(Some(id)) => {
                        info!(%id, tool = %result.tool_name, agent = %result.agent_id, "tool result attached");
                    }
                    Ok(None) => {
                        warn!(tool = %result.tool_name, agent = %result.agent_id,
                              "tool result dropped: no matching decision appeared within the retry window");
                    }
                    Err(e) => {
                        warn!("failed to store tool result: {e}");
                    }
                }
            });
        }
        ClientMessage::AgentRegister {
            agent_id,
            agent_type,
            project,
        } => {
            // Fire-and-forget registration (no response)
            let (info, is_new) = {
                let mut reg = ctx.agent_registry.lock().await;
                reg.register(agent_id, agent_type, project)
            };
            if is_new {
                let _ = ctx.tui_tx.send(ServerMessage::AgentConnected(info));
            }
        }
        _ => {
            write_msg(
                &mut writer,
                &ServerMessage::Error {
                    message: "expected DecisionRequest, ToolResult, or AgentRegister from hook"
                        .into(),
                },
            )
            .await?;
        }
    }

    Ok(())
}

/// A pending bulk-approve item: the queued decision id plus its tool name.
type GatedItem = (uuid::Uuid, String);

/// Partition queued `(id, tool_name)` items into `(gated, allowed)` for a
/// web-origin bulk approve.
///
/// When the device's sudo grace is `fresh`, nothing is held back — every item
/// is allowed. Otherwise sudo-class tools (`is_sudo_tool`) are split off as
/// `gated` (they get a `WebReauthRequired` bounce) and the rest are allowed.
/// Pure decision logic lifted verbatim from the `ApproveAll` arm so it can be
/// unit-tested in isolation.
fn partition_sudo_gated(fresh: bool, matching: Vec<GatedItem>) -> (Vec<GatedItem>, Vec<GatedItem>) {
    if fresh {
        (Vec::new(), matching)
    } else {
        matching
            .into_iter()
            .partition(|(_, t)| crate::sudo_gate::is_sudo_tool(t))
    }
}

/// Encode a single `ServerMessage` and write it to the TUI socket.
///
/// Dedupes the `let encoded = encode(&msg)?; writer.write_all(...)` pair that
/// previously appeared dozens of times across the TUI handler. Behaviour is
/// identical to the inline form: on the happy path it returns `Ok(())`; an
/// encode or write error is propagated to the caller, which decides whether to
/// break the select loop (on disconnect) or bubble the error up.
async fn write_msg(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    msg: &ServerMessage,
) -> Result<()> {
    let encoded = encode(msg)?;
    writer.write_all(encoded.as_bytes()).await?;
    Ok(())
}

/// Audit-trail identity of the resolving client (itr#88): the local TUI runs
/// with the daemon's own uid over the peer-checked socket, web clients carry
/// their authenticated device id.
fn resolver_label(device_id: &Option<wisphive_protocol::DeviceId>) -> String {
    match device_id {
        Some(dev) => format!("human:web:{}", dev.0),
        None => "human:tui".to_string(),
    }
}

/// Eagerly persist a resolved decision so subsequent history queries see it.
///
/// The hook handler's `resolve_pending` is idempotent (no-op if already done),
/// so resolving here ahead of the hook is safe. A persistence failure is logged
/// and swallowed — exactly as the six inline call sites did before extraction.
/// `resolver` lands in decision_log.decided_by (itr#88).
async fn eager_persist(
    state_db: &crate::state::StateDb,
    id: uuid::Uuid,
    decision: Decision,
    resolver: &str,
) {
    if let Err(e) = state_db.resolve_pending_by(id, decision, resolver).await {
        warn!("eager persist failed for {id}: {e}");
    }
}

/// Handle a TUI connection: stream events, receive commands.
async fn handle_tui(
    mut lines: SocketReader,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
) -> Result<()> {
    use tokio::sync::mpsc;

    // Send agents snapshot
    let agents_snap = {
        let reg = ctx.agent_registry.lock().await;
        reg.snapshot()
    };
    write_msg(
        &mut writer,
        &ServerMessage::AgentsSnapshot {
            agents: agents_snap,
        },
    )
    .await?;

    // Send initial queue snapshot
    let snapshot = {
        let q = ctx.queue.lock().await;
        q.snapshot()
    };
    write_msg(
        &mut writer,
        &ServerMessage::QueueSnapshot { items: snapshot },
    )
    .await?;

    // Subscribe to broadcast events for this TUI
    let mut tui_rx = ctx.tui_tx.subscribe();

    // Per-connection channel for messages produced by worker tasks
    // (e.g. per-session terminal forwarders). The select loop drains this
    // and writes to the single owned socket, so there's no lock contention
    // on the writer.
    // Bounded (itr#82): an unbounded channel let a fast producer (a chatty
    // terminal forwarder or a large `TermReplay`) outrun a slow/stalled TUI
    // socket and grow the queue without limit → OOM → daemon crash → gating
    // disabled. 1024 caps worst-case memory while absorbing a healthy burst;
    // producers `try_send` and drop on `Full` (see the forwarders below).
    let (conn_tx, mut conn_rx) = mpsc::channel::<ServerMessage>(CONN_CHANNEL_CAPACITY);

    // Attached terminal sessions on this connection. Aborted on detach,
    // disconnect, or TermEnded. Key: terminal session id.
    let mut term_attachments: std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    // Persistent partial-line accumulator for the capped command reader (itr#83).
    // Lives across loop iterations so a cancelled read (sibling select branch
    // fired) resumes its partial line instead of desyncing the framing.
    let mut line_buf = Vec::new();

    loop {
        tokio::select! {
            // Per-connection messages from worker tasks (e.g. terminal forwarders)
            msg = conn_rx.recv() => {
                match msg {
                    Some(m) => {
                        if write_msg(&mut writer, &m).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            // Forward daemon events to TUI
            event = tui_rx.recv() => {
                match event {
                    Ok(msg) => {
                        if write_msg(&mut writer, &msg).await.is_err() {
                            break; // TUI disconnected
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("TUI lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Read commands from TUI
            line = read_capped_line(&mut lines, &mut line_buf) => {
                match line {
                    Ok(Some(text)) => {
                        let command: wisphive_protocol::ClientCommand =
                            match wisphive_protocol::decode(&text) {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!("invalid TUI message: {e}");
                                    continue;
                                }
                            };
                        // Every decision arm logs `%device_id` (None = local TUI,
                        // implicitly trusted). The Approve / ApproveAll arms
                        // consult `ctx.reauth` before honouring web-origin
                        // approvals of sudo-class tools (itr#218).
                        let device_id = command.device_id.clone();
                        let msg = command.body;
                        if dispatch_command(
                            &mut writer,
                            ctx,
                            device_id,
                            msg,
                            &mut term_attachments,
                            &conn_tx,
                        )
                        .await?
                        .is_break()
                        {
                            break;
                        }
                    }
                    Ok(None) => break, // TUI disconnected
                    Err(e) => {
                        warn!("TUI read error: {e}");
                        break;
                    }
                }
            }
        }
    }

    // Abort any attached terminal forwarders tied to this connection so
    // they stop trying to send down a dead channel.
    for (_, handle) in term_attachments.drain() {
        handle.abort();
    }

    info!("TUI client disconnected");
    Ok(())
}

/// Route one decoded TUI command to its per-domain dispatcher.
///
/// Returns `ControlFlow::Break(())` only when a dispatcher signals the select
/// loop should stop (TUI disconnect); `ControlFlow::Continue(())` otherwise.
/// Pure routing — the behaviour of each arm is unchanged from the original
/// inline `match`.
async fn dispatch_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
    device_id: Option<wisphive_protocol::DeviceId>,
    msg: ClientMessage,
    term_attachments: &mut std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>>,
    conn_tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<std::ops::ControlFlow<()>> {
    use std::ops::ControlFlow;
    match msg {
        ClientMessage::Approve { .. }
        | ClientMessage::Deny { .. }
        | ClientMessage::Ask { .. }
        | ClientMessage::ApproveAll { .. }
        | ClientMessage::DenyAll { .. }
        | ClientMessage::ApprovePermission { .. }
        | ClientMessage::MarkDeviceFresh => {
            handle_decision_command(writer, ctx, device_id, msg).await?;
        }
        ClientMessage::SpawnAgent(_)
        | ClientMessage::ListAgents
        | ClientMessage::StopAgent { .. }
        | ClientMessage::ReimportEvents => {
            handle_agent_command(writer, ctx, msg).await?;
        }
        ClientMessage::QueryHistory { .. }
        | ClientMessage::SearchHistory(_)
        | ClientMessage::QuerySessions
        | ClientMessage::QueryProjects => {
            handle_query_command(writer, ctx, msg).await?;
        }
        ClientMessage::TermCreate { .. }
        | ClientMessage::TermAttach { .. }
        | ClientMessage::TermDetach { .. }
        | ClientMessage::TermInput { .. }
        | ClientMessage::TermResize { .. }
        | ClientMessage::TermClose { .. }
        | ClientMessage::TermList
        | ClientMessage::TermSetGroup { .. }
        | ClientMessage::TermReorder { .. }
        | ClientMessage::TermReplay { .. } => {
            handle_terminal_command(writer, ctx, msg, term_attachments, conn_tx).await?;
        }
        _ => {
            warn!("unexpected message from TUI: {:?}", msg);
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Dispatch decision-class commands: approve/deny/ask, bulk variants, the
/// permission-selection approve, and the web sudo reauth refresh.
///
/// These share the sudo/reauth gate (itr#218) and the eager-persist path.
async fn handle_decision_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
    device_id: Option<wisphive_protocol::DeviceId>,
    msg: ClientMessage,
) -> Result<()> {
    match msg {
        ClientMessage::Approve {
            id,
            message,
            updated_input,
            always_allow,
            additional_context,
        } => {
            info!(?device_id, %id, "approve");

            // Sudo-mode gate: if the approve is coming from
            // an authenticated web device, the target tool is
            // sudo-class, and the device's reauth grace has
            // lapsed, we refuse to resolve and bounce a
            // WebReauthRequired back on this connection so
            // the browser can open the sudo modal. TUI
            // origin approvals (device_id = None) bypass.
            if let Some(ref dev) = device_id {
                let tool = {
                    let q = ctx.queue.lock().await;
                    q.peek(id).map(|r| r.tool_name.clone())
                };
                if let Some(tool_name) = tool
                    && crate::sudo_gate::is_sudo_tool(&tool_name)
                    && !ctx.reauth.is_fresh(dev).await
                {
                    let reauth_msg = ServerMessage::WebReauthRequired {
                        device_id: dev.0.clone(),
                        request_id: id.to_string(),
                        tool_name: tool_name.clone(),
                        at: chrono::Utc::now(),
                    };
                    write_msg(writer, &reauth_msg).await?;
                    debug!(%id, tool = %tool_name, device_id = %dev.0, "sudo gate: reauth required");
                    return Ok(());
                }
            }

            let rich = RichDecision {
                decision: Decision::Approve,
                message,
                updated_input,
                always_allow,
                additional_context,
                selected_permission: None,
                resolver: Some(resolver_label(&device_id)),
            };
            let resolved = {
                let mut q = ctx.queue.lock().await;
                q.resolve(id, rich)
            };
            // Eagerly persist so subsequent history queries see this decision —
            // but only when the item was actually pending. A stale resolve
            // (already timed out / abandoned, itr#363) must not write an audit
            // row contradicting the recorded outcome.
            if resolved {
                eager_persist(
                    &ctx.state_db,
                    id,
                    Decision::Approve,
                    &resolver_label(&device_id),
                )
                .await;
            }
        }
        ClientMessage::Deny { id, message } => {
            info!(?device_id, %id, "deny");
            let rich = RichDecision {
                decision: Decision::Deny,
                message,
                resolver: Some(resolver_label(&device_id)),
                ..RichDecision::deny()
            };
            let resolved = {
                let mut q = ctx.queue.lock().await;
                q.resolve(id, rich)
            };
            if resolved {
                eager_persist(
                    &ctx.state_db,
                    id,
                    Decision::Deny,
                    &resolver_label(&device_id),
                )
                .await;
            }
        }
        ClientMessage::Ask { id } => {
            info!(?device_id, %id, "ask");
            let mut q = ctx.queue.lock().await;
            q.resolve(id, RichDecision::from(Decision::Ask));
            // Ask/defer decisions are not persisted to the audit log
        }
        ClientMessage::ApproveAll {
            ref filter,
            confirm,
        } => {
            // An UNFILTERED bulk approve requires explicit confirmation
            // (itr#88): a buggy or compromised client echoing NewDecision
            // events must not be able to blanket-approve the whole queue with
            // one message. The TUI sends confirm=true after its Y/N modal.
            if filter.is_none() && !confirm {
                warn!(
                    ?device_id,
                    "rejected unfiltered approve_all without confirm (itr#88)"
                );
                write_msg(
                    writer,
                    &ServerMessage::Error {
                        message: "approve_all without a filter requires confirm=true".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            // Web-origin bulk approves get the same sudo-class
            // treatment as single approvals: items in the
            // sudo-class set are held back behind a
            // WebReauthRequired signal while the rest
            // resolve. TUI-origin bulk approves are trusted
            // and fast-path through the original path.
            if let Some(ref dev) = device_id {
                let matching: Vec<(uuid::Uuid, String)> = {
                    let q = ctx.queue.lock().await;
                    q.snapshot()
                        .into_iter()
                        .filter(|req| filter.as_ref().is_none_or(|f| f.matches(req)))
                        .map(|req| (req.id, req.tool_name))
                        .collect()
                };
                let fresh = ctx.reauth.is_fresh(dev).await;
                let (gated, allowed) = partition_sudo_gated(fresh, matching);

                let allowed_ids: Vec<uuid::Uuid> = {
                    let mut q = ctx.queue.lock().await;
                    allowed
                        .iter()
                        .filter(|(id, _)| {
                            let rich = RichDecision {
                                resolver: Some(resolver_label(&device_id)),
                                ..RichDecision::from(Decision::Approve)
                            };
                            q.resolve(*id, rich)
                        })
                        .map(|(id, _)| *id)
                        .collect()
                };
                info!(
                    ?device_id,
                    approved = allowed_ids.len(),
                    gated = gated.len(),
                    "approve_all"
                );
                for id in &allowed_ids {
                    eager_persist(
                        &ctx.state_db,
                        *id,
                        Decision::Approve,
                        &resolver_label(&device_id),
                    )
                    .await;
                }
                for (id, tool_name) in gated {
                    let reauth_msg = ServerMessage::WebReauthRequired {
                        device_id: dev.0.clone(),
                        request_id: id.to_string(),
                        tool_name: tool_name.clone(),
                        at: chrono::Utc::now(),
                    };
                    write_msg(writer, &reauth_msg).await?;
                    debug!(%id, tool = %tool_name, device_id = %dev.0, "sudo gate: reauth required (approve_all)");
                }
            } else {
                let ids = {
                    let mut q = ctx.queue.lock().await;
                    q.resolve_all(filter, Decision::Approve, Some(&resolver_label(&device_id)))
                };
                info!(?device_id, count = ids.len(), "approve_all");
                for id in ids {
                    eager_persist(
                        &ctx.state_db,
                        id,
                        Decision::Approve,
                        &resolver_label(&device_id),
                    )
                    .await;
                }
            }
        }
        ClientMessage::DenyAll { ref filter } => {
            let ids = {
                let mut q = ctx.queue.lock().await;
                q.resolve_all(filter, Decision::Deny, Some(&resolver_label(&device_id)))
            };
            info!(?device_id, count = ids.len(), "deny_all");
            for id in ids {
                eager_persist(
                    &ctx.state_db,
                    id,
                    Decision::Deny,
                    &resolver_label(&device_id),
                )
                .await;
            }
        }
        ClientMessage::MarkDeviceFresh => {
            // Only authenticated web origins can refresh sudo
            // timers — the envelope's device_id is filled in
            // by the web crate's short-lived sender
            // (post_auth_reauth), never by the browser. A
            // payload without a device_id is dropped so a
            // local TUI bug can't accidentally elevate any
            // phantom device.
            if let Some(ref dev) = device_id {
                ctx.reauth.touch(dev).await;
                info!(device_id = %dev.0, "device marked fresh for sudo-mode");
                write_msg(
                    writer,
                    &ServerMessage::MarkDeviceFreshAck {
                        device_id: dev.0.clone(),
                    },
                )
                .await?;
            } else {
                warn!("mark_device_fresh without device_id; ignoring");
            }
        }
        ClientMessage::ApprovePermission {
            id,
            suggestion_index,
            message,
        } => {
            info!(?device_id, %id, suggestion_index, "approve_permission");
            // Look up the selected suggestion from the queued request
            let selected = {
                let q = ctx.queue.lock().await;
                q.snapshot()
                    .iter()
                    .find(|r| r.id == id)
                    .and_then(|r| r.permission_suggestions.as_ref())
                    .and_then(|s| s.get(suggestion_index))
                    .cloned()
            };
            // Fail closed on a bad index (itr#297): approving with
            // selected_permission=None would grant the call without any
            // permission actually chosen. Leave the request pending.
            if selected.is_none() {
                warn!(?device_id, %id, suggestion_index, "approve_permission with invalid suggestion index rejected");
                write_msg(
                    writer,
                    &ServerMessage::Error {
                        message: format!(
                            "invalid suggestion_index {suggestion_index} for decision {id}; request left pending"
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            let rich = RichDecision {
                decision: Decision::Approve,
                message,
                updated_input: None,
                always_allow: false,
                additional_context: None,
                selected_permission: selected,
                resolver: Some(resolver_label(&device_id)),
            };
            {
                let mut q = ctx.queue.lock().await;
                q.resolve(id, rich);
            }
            eager_persist(
                &ctx.state_db,
                id,
                Decision::Approve,
                &resolver_label(&device_id),
            )
            .await;
        }
        _ => {
            warn!("unexpected message from TUI: {:?}", msg);
        }
    }
    Ok(())
}

/// Dispatch agent-process commands: spawn, list, stop, and event re-import.
async fn handle_agent_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
    msg: ClientMessage,
) -> Result<()> {
    match msg {
        ClientMessage::SpawnAgent(req) => {
            let mut pr = ctx.process_registry.lock().await;
            match pr.spawn_agent(req).await {
                Ok(agent) => {
                    write_msg(writer, &ServerMessage::AgentSpawned(agent)).await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("failed to spawn agent: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::ListAgents => {
            let pr = ctx.process_registry.lock().await;
            let agents = pr.list();
            write_msg(writer, &ServerMessage::AgentList { agents }).await?;
        }
        ClientMessage::ReimportEvents => {
            let events_path = ctx.home_dir.join("events.jsonl");
            match crate::event_ingest::reimport_all(&events_path, &ctx.state_db).await {
                Ok(count) => {
                    write_msg(writer, &ServerMessage::ReimportComplete { count }).await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("reimport failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::StopAgent { ref agent_id } => {
            let mut pr = ctx.process_registry.lock().await;
            match pr.stop_agent(agent_id).await {
                Ok(exit_code) => {
                    write_msg(
                        writer,
                        &ServerMessage::AgentExited {
                            agent_id: agent_id.clone(),
                            exit_code,
                        },
                    )
                    .await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("{e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        _ => {
            warn!("unexpected message from TUI: {:?}", msg);
        }
    }
    Ok(())
}

/// Dispatch history/session/project query commands. Sessions and projects are
/// enriched with live agent presence and pending counts before responding.
async fn handle_query_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
    msg: ClientMessage,
) -> Result<()> {
    match msg {
        ClientMessage::QueryHistory {
            ref agent_id,
            limit,
            ref request_id,
        } => {
            let limit = limit.unwrap_or(200);
            match ctx.state_db.query_history(agent_id.as_deref(), limit).await {
                Ok(entries) => {
                    write_msg(
                        writer,
                        &ServerMessage::HistoryResponse {
                            entries,
                            request_id: request_id.clone(),
                        },
                    )
                    .await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("history query failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::SearchHistory(ref search) => {
            match ctx.state_db.search_history(search).await {
                Ok(entries) => {
                    write_msg(
                        writer,
                        &ServerMessage::HistoryResponse {
                            entries,
                            request_id: search.request_id.clone(),
                        },
                    )
                    .await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("search failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::QuerySessions => {
            match ctx.state_db.query_sessions().await {
                Ok(mut sessions) => {
                    // Enrich with live status
                    let live_agents = {
                        let reg = ctx.agent_registry.lock().await;
                        reg.snapshot()
                    };
                    let live_ids: std::collections::HashSet<String> =
                        live_agents.iter().map(|a| a.agent_id.clone()).collect();

                    // Pending counts from queue
                    let pending_counts: std::collections::HashMap<String, u32> = {
                        let q = ctx.queue.lock().await;
                        let snapshot = q.snapshot();
                        let mut counts = std::collections::HashMap::new();
                        for req in &snapshot {
                            *counts.entry(req.agent_id.clone()).or_insert(0) += 1;
                        }
                        counts
                    };

                    for session in &mut sessions {
                        session.is_live = live_ids.contains(&session.agent_id);
                        session.pending_count =
                            pending_counts.get(&session.agent_id).copied().unwrap_or(0);
                    }

                    // Add live agents with no history yet
                    for agent in &live_agents {
                        if !sessions.iter().any(|s| s.agent_id == agent.agent_id) {
                            sessions.push(wisphive_protocol::SessionSummary {
                                agent_id: agent.agent_id.clone(),
                                agent_type: agent.agent_type.clone(),
                                project: agent.project.clone(),
                                first_seen: agent.connected_at,
                                last_seen: agent.last_seen,
                                total_calls: 0,
                                approved: 0,
                                denied: 0,
                                is_live: true,
                                pending_count: pending_counts
                                    .get(&agent.agent_id)
                                    .copied()
                                    .unwrap_or(0),
                            });
                        }
                    }

                    // Sort: live+pending first, then live, then by last_seen DESC
                    sessions.sort_by(|a, b| {
                        let a_key = (a.is_live && a.pending_count > 0, a.is_live, a.last_seen);
                        let b_key = (b.is_live && b.pending_count > 0, b.is_live, b.last_seen);
                        b_key
                            .partial_cmp(&a_key)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });

                    write_msg(writer, &ServerMessage::SessionsResponse { sessions }).await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("sessions query failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::QueryProjects => {
            match ctx.state_db.query_projects().await {
                Ok(mut projects) => {
                    // Enrich with live agent presence
                    let live_agents = {
                        let reg = ctx.agent_registry.lock().await;
                        reg.snapshot()
                    };
                    let mut live_projects: std::collections::HashSet<std::path::PathBuf> =
                        std::collections::HashSet::new();
                    for agent in &live_agents {
                        live_projects.insert(agent.project.clone());
                    }

                    // Pending counts per project
                    let pending_counts: std::collections::HashMap<std::path::PathBuf, u32> = {
                        let q = ctx.queue.lock().await;
                        let snapshot = q.snapshot();
                        let mut counts = std::collections::HashMap::new();
                        for req in &snapshot {
                            *counts.entry(req.project.clone()).or_insert(0) += 1;
                        }
                        counts
                    };

                    for project in &mut projects {
                        project.has_live_agents = live_projects.contains(&project.project);
                        project.pending_count =
                            pending_counts.get(&project.project).copied().unwrap_or(0);
                    }

                    // Add projects with live agents but no history
                    for agent in &live_agents {
                        if !projects.iter().any(|p| p.project == agent.project) {
                            projects.push(wisphive_protocol::ProjectSummary {
                                project: agent.project.clone(),
                                first_seen: agent.connected_at,
                                last_seen: agent.last_seen,
                                total_calls: 0,
                                approved: 0,
                                denied: 0,
                                agent_count: 1,
                                pending_count: pending_counts
                                    .get(&agent.project)
                                    .copied()
                                    .unwrap_or(0),
                                has_live_agents: true,
                            });
                        }
                    }

                    projects.sort_by(|a, b| {
                        let a_key = (
                            a.has_live_agents && a.pending_count > 0,
                            a.has_live_agents,
                            a.last_seen,
                        );
                        let b_key = (
                            b.has_live_agents && b.pending_count > 0,
                            b.has_live_agents,
                            b.last_seen,
                        );
                        b_key
                            .partial_cmp(&a_key)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });

                    write_msg(writer, &ServerMessage::ProjectsResponse { projects }).await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::Error {
                            message: format!("projects query failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        _ => {
            warn!("unexpected message from TUI: {:?}", msg);
        }
    }
    Ok(())
}

/// Dispatch terminal-session commands. These mutate the per-connection
/// `term_attachments` map and spawn forwarders that send back over `conn_tx`.
async fn handle_terminal_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
    msg: ClientMessage,
    term_attachments: &mut std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>>,
    conn_tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<()> {
    match msg {
        ClientMessage::TermCreate {
            label,
            command,
            args,
            cwd,
            cols,
            rows,
            env,
        } => {
            match ctx
                .terminal_manager
                .create(label, command, args, cwd, cols, rows, env)
                .await
            {
                Ok(meta) => {
                    write_msg(writer, &ServerMessage::TermCreated(meta)).await?;
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::TermError {
                            id: None,
                            message: format!("term create failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::TermAttach { id } => {
            if let Some(handle) = term_attachments.remove(&id) {
                handle.abort();
            }
            let session = ctx.terminal_manager.get(id).await;
            match session {
                Some(session) => {
                    // Snapshot the current screen BEFORE subscribing so
                    // the seq counter we capture matches what we'll see
                    // on the receiver.
                    let next_seq = session.seq_load();
                    let catchup = crate::terminal::catchup_message(&session, next_seq);
                    write_msg(writer, &catchup).await?;

                    let mut rx = session.subscribe();
                    let sess_id = session.id;
                    let tx = conn_tx.clone();
                    let handle = tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(frame) => {
                                    if frame.seq < next_seq {
                                        continue;
                                    }
                                    let msg = crate::terminal::frame_to_chunk(sess_id, &frame);
                                    // Bounded channel (itr#82): drop a frame on `Full`
                                    // (slow/stalled TUI) rather than block this forwarder
                                    // or grow memory; only a closed channel ends it.
                                    match tx.try_send(msg) {
                                        Ok(()) => {}
                                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                            warn!(%sess_id, "TUI conn channel full; dropping terminal frame");
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                            break;
                                        }
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => {
                                    let _ = tx.try_send(ServerMessage::TermError {
                                        id: Some(sess_id),
                                        message: "attachment lagged, please re-attach".into(),
                                    });
                                    break;
                                }
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                    term_attachments.insert(id, handle);
                }
                None => {
                    write_msg(
                        writer,
                        &ServerMessage::TermError {
                            id: Some(id),
                            message: "terminal session not found or no longer running".into(),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::TermDetach { id } => {
            if let Some(handle) = term_attachments.remove(&id) {
                handle.abort();
            }
        }
        ClientMessage::TermInput { id, data } => match crate::terminal::decode_b64(&data) {
            Ok(bytes) => {
                if let Err(e) = ctx.terminal_manager.write_input(id, bytes).await {
                    write_msg(
                        writer,
                        &ServerMessage::TermError {
                            id: Some(id),
                            message: format!("term input failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
            Err(e) => {
                write_msg(
                    writer,
                    &ServerMessage::TermError {
                        id: Some(id),
                        message: format!("invalid term input payload: {e}"),
                    },
                )
                .await?;
            }
        },
        ClientMessage::TermResize { id, cols, rows } => {
            if let Err(e) = ctx.terminal_manager.resize(id, cols, rows).await {
                write_msg(
                    writer,
                    &ServerMessage::TermError {
                        id: Some(id),
                        message: format!("term resize failed: {e}"),
                    },
                )
                .await?;
            }
        }
        ClientMessage::TermClose { id, kill } => {
            if let Some(handle) = term_attachments.remove(&id) {
                handle.abort();
            }
            if let Err(e) = ctx.terminal_manager.close(id, kill).await {
                write_msg(
                    writer,
                    &ServerMessage::TermError {
                        id: Some(id),
                        message: format!("term close failed: {e}"),
                    },
                )
                .await?;
            }
        }
        ClientMessage::TermList => match ctx.terminal_manager.list_all().await {
            Ok(sessions) => {
                write_msg(writer, &ServerMessage::TermListResponse { sessions }).await?;
            }
            Err(e) => {
                write_msg(
                    writer,
                    &ServerMessage::TermError {
                        id: None,
                        message: format!("term list failed: {e}"),
                    },
                )
                .await?;
            }
        },
        ClientMessage::TermSetGroup { id, group } => {
            match ctx.terminal_manager.set_group(id, group.as_deref()).await {
                Ok(()) => {
                    if let Ok(sessions) = ctx.terminal_manager.list_all().await {
                        let _ = ctx
                            .tui_tx
                            .send(ServerMessage::TermListResponse { sessions });
                    }
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::TermError {
                            id: Some(id),
                            message: format!("term set group failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::TermReorder { id, sort_order } => {
            match ctx.terminal_manager.set_sort_order(id, sort_order).await {
                Ok(()) => {
                    if let Ok(sessions) = ctx.terminal_manager.list_all().await {
                        let _ = ctx
                            .tui_tx
                            .send(ServerMessage::TermListResponse { sessions });
                    }
                }
                Err(e) => {
                    write_msg(
                        writer,
                        &ServerMessage::TermError {
                            id: Some(id),
                            message: format!("term reorder failed: {e}"),
                        },
                    )
                    .await?;
                }
            }
        }
        ClientMessage::TermReplay {
            id,
            from_seq,
            speed: _,
        } => {
            // Pull events from SQLite and stream them as
            // replay chunks. Speed pacing is client-side.
            let state_db = ctx.state_db.clone();
            let tx = conn_tx.clone();
            tokio::spawn(async move {
                match state_db.replay_terminal_events(id, from_seq).await {
                    Ok(events) => {
                        let total = events.len() as u64;
                        for (seq, ts_us, direction, payload) in events {
                            let msg = ServerMessage::TermReplayChunk {
                                id,
                                seq,
                                ts_us,
                                direction,
                                data: base64::Engine::encode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &payload,
                                ),
                            };
                            if tx.send(msg).await.is_err() {
                                return;
                            }
                        }
                        let _ = tx
                            .send(ServerMessage::TermReplayDone {
                                id,
                                total_events: total,
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(ServerMessage::TermError {
                                id: Some(id),
                                message: format!("replay failed: {e}"),
                            })
                            .await;
                    }
                }
            });
        }
        _ => {
            warn!("unexpected message from TUI: {:?}", msg);
        }
    }
    Ok(())
}

/// Retry schedule for attaching a tool result whose decision row may still be
/// in the JSONL ingest pipeline (itr#302): a fast tool can report PostToolUse
/// before the async ingester inserts its auto-approved decision row, and a
/// single immediate attempt permanently dropped the result from history. The
/// total window (~14s) bounds how long an unmatched result lingers — orders of
/// magnitude beyond notify-based ingest latency, and one detached task per
/// unmatched result keeps memory bounded.
const ATTACH_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(10),
];

/// Attach a tool result, retrying per `delays` while no decision row matches.
/// Returns the matched decision id, or `None` once the schedule is exhausted.
async fn attach_tool_result_with_retry(
    state_db: &StateDb,
    result: &wisphive_protocol::ToolResult,
    delays: impl IntoIterator<Item = Duration>,
) -> Result<Option<uuid::Uuid>> {
    let attach = || {
        state_db.attach_tool_result(
            &result.agent_id,
            &result.tool_name,
            &result.tool_result,
            result.tool_use_id.as_deref(),
        )
    };
    if let Some(id) = attach().await? {
        return Ok(Some(id));
    }
    for delay in delays {
        tokio::time::sleep(delay).await;
        if let Some(id) = attach().await? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Persist an "Always Allow" choice so the hook honors it on the next call.
///
/// Writes `auto_approve_add` in ~/.wisphive/config.json via an atomic raw-JSON
/// read-modify-write (unknown keys survive), because the hook stops consulting
/// the legacy auto-approve.json as soon as config.json carries a parseable
/// `auto_approve_level` (itr#360). The tool is also dropped from
/// `auto_approve_remove`, which the hook checks first and which would otherwise
/// veto the addition. Falls back to the legacy file only when config.json is
/// absent; a corrupt config.json refuses the update rather than clobbering it
/// (itr#308).
fn persist_auto_approve(tool_name: &str, wisphive_dir: &std::path::Path) -> Result<()> {
    let config_path = wisphive_dir.join("config.json");
    if config_path.exists() {
        crate::config::update_config_json(&config_path, |obj| {
            let add = obj
                .entry("auto_approve_add")
                .or_insert(serde_json::json!([]));
            // A wrong-typed existing value is an error, not a silent success —
            // the hook would never honor the "addition" (itr#308 posture).
            let arr = add
                .as_array_mut()
                .ok_or_else(|| "auto_approve_add exists but is not an array".to_string())?;
            if !arr.iter().any(|v| v.as_str() == Some(tool_name)) {
                arr.push(serde_json::Value::String(tool_name.to_string()));
            }
            if let Some(arr) = obj
                .get_mut("auto_approve_remove")
                .and_then(|v| v.as_array_mut())
            {
                arr.retain(|v| v.as_str() != Some(tool_name));
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("config.json: {e}"))?;
        info!(tool = tool_name, "added to auto_approve_add in config.json");
        return Ok(());
    }

    // No config.json — the hook is on the legacy/defaults path, so the legacy
    // file is what it will actually read.
    let path = wisphive_dir.join("auto-approve.json");
    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        // A corrupt legacy file is refused, not silently replaced with `{}`
        // and rewritten (itr#308).
        serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("auto-approve.json is not valid JSON ({e}); refusing to overwrite it")
        })?
    } else {
        serde_json::json!({})
    };

    let arr = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("auto-approve.json is not an object"))?
        .entry("auto_approve")
        .or_insert(serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("auto_approve is not an array"))?;

    if !arr.iter().any(|v| v.as_str() == Some(tool_name)) {
        arr.push(serde_json::Value::String(tool_name.to_string()));
        info!(tool = tool_name, "added to legacy auto-approve list");
    }

    crate::config::write_config_atomic(&path, &serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn sweep_stale_session_markers(sessions_dir: &std::path::Path) {
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(path = %sessions_dir.display(), error = %e, "session sweep: read_dir failed");
            return;
        }
    };

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "session sweep: remove failed");
            }
        }
    }

    if removed > 0 {
        info!(removed, "session sweep: cleared stale markers");
    }
}

/// Force the bound Unix socket to owner-only (0600) permissions.
///
/// Called right after `bind`, before the accept loop starts, so no client
/// can connect while the socket is still at the umask-derived mode. Uses
/// `set_permissions` (a `chmod` on the path) — the socket already exists at
/// this point, so there's no create-time race with a non-owner (only the
/// owning uid could have created an entry at this path).
fn set_socket_permissions(socket_path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
}

/// The effective uid of the running daemon process.
fn current_uid() -> u32 {
    // SAFETY: `geteuid` is always-succeeds and has no preconditions.
    unsafe { libc::geteuid() }
}

/// Decide whether a connecting peer's uid is allowed to drive the daemon.
///
/// Only the daemon's own uid is trusted; every other local uid is rejected.
/// `root` (uid 0) is deliberately NOT special-cased as allowed — a root peer
/// that is not the daemon's own uid is still a foreign principal under this
/// single-user model and is dropped. Pure function so the accept/reject
/// decision is unit-testable without a cross-uid socket.
fn peer_uid_allowed(peer_uid: u32, daemon_uid: u32) -> bool {
    peer_uid == daemon_uid
}

#[cfg(test)]
mod tests {
    use super::{
        CONN_CHANNEL_CAPACITY, MAX_LINE_BYTES, attach_tool_result_with_retry, partition_sudo_gated,
        peer_uid_allowed, persist_auto_approve, read_capped_line, set_socket_permissions,
        sweep_stale_session_markers,
    };
    use tokio::io::BufReader;

    fn tool_result(tool_use_id: &str) -> wisphive_protocol::ToolResult {
        wisphive_protocol::ToolResult {
            agent_id: "cc-1".into(),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_result: serde_json::json!({"output": "ok"}),
            timestamp: chrono::Utc::now(),
            tool_use_id: Some(tool_use_id.into()),
        }
    }

    fn auto_approved_line(tool_use_id: &str) -> String {
        serde_json::json!({
            "event": "auto_approved",
            "agent_id": "cc-1",
            "agent_type": "claude_code",
            "project": "/test",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "timestamp": "2024-01-01T00:00:00Z",
            "tool_use_id": tool_use_id,
        })
        .to_string()
    }

    #[tokio::test]
    async fn restart_drains_orphaned_pending_as_failopen() {
        // itr#299: a pending row a crashed daemon never resolved is drained on
        // the next Server::new() and recorded as the truthful fail-open outcome
        // (the blocked hook already ran the tool via DaemonUnreachable).
        use crate::DaemonConfig;
        use crate::state::StateDb;
        use crate::state::test_support::make_request;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();

        // First boot: create dirs + schema, nothing to drain.
        let s1 = super::Server::new(DaemonConfig::new(home.clone())).await.unwrap();
        let db_path = DaemonConfig::new(home.clone())
            .db_path
            .to_string_lossy()
            .to_string();
        drop(s1);

        // Leave an unresolved in-flight decision behind (the crash).
        let req = make_request("Bash", "cc-1", "/muse");
        {
            let db = StateDb::open(&db_path).await.unwrap();
            db.persist_pending(&req).await.unwrap();
            assert_eq!(db.pending_count().await.unwrap(), 1);
        }

        // Restart: Server::new must drain the orphan.
        let _s2 = super::Server::new(DaemonConfig::new(home.clone())).await.unwrap();

        let db = StateDb::open(&db_path).await.unwrap();
        assert_eq!(db.pending_count().await.unwrap(), 0, "restart must clear orphans");
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].decision, wisphive_protocol::Decision::Approve);
        assert_eq!(
            history[0].decided_by.as_deref(),
            Some("daemon_restart:failopen")
        );
    }

    #[tokio::test]
    async fn tool_result_attaches_after_ingest_via_retry() {
        // itr#302: a fast tool's PostToolUse can beat the async JSONL ingester;
        // the retry schedule must attach the result once the decision row lands.
        let db = crate::state::StateDb::open(":memory:").await.unwrap();
        let result = tool_result("race-1");

        // No decision row yet — an empty schedule (single attempt) drops it.
        let attached = attach_tool_result_with_retry(&db, &result, [])
            .await
            .unwrap();
        assert!(attached.is_none(), "no row yet — single attempt must miss");

        // Row lands mid-schedule; the retry picks it up.
        let ingest = async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            crate::event_ingest::ingest_line(&auto_approved_line("race-1"), &db)
                .await
                .unwrap();
        };
        let retry = attach_tool_result_with_retry(
            &db,
            &result,
            [
                std::time::Duration::from_millis(20),
                std::time::Duration::from_millis(100),
                std::time::Duration::from_millis(200),
            ],
        );
        let (attached, ()) = tokio::join!(retry, ingest);
        assert!(
            attached.unwrap().is_some(),
            "result must attach once the ingested row appears"
        );

        // And the result is really on the row.
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].tool_result.is_some());
    }

    #[test]
    fn always_allow_writes_config_json_when_present() {
        // itr#360: once config.json carries an auto_approve_level, the hook
        // never consults the legacy auto-approve.json — 'Always Allow' must
        // land in config.json's auto_approve_add or it does nothing.
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(
            &config_path,
            serde_json::json!({
                "auto_approve_level": "read",
                "auto_approve_remove": ["Bash"],
                "tool_rules": {"Bash": {"deny_patterns": ["sudo"], "allow_patterns": []}},
            })
            .to_string(),
        )
        .unwrap();

        persist_auto_approve("Bash", dir.path()).unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(after["auto_approve_add"][0], "Bash");
        // The removal veto is lifted, or the addition would be dead on arrival.
        assert!(after["auto_approve_remove"].as_array().unwrap().is_empty());
        // Untouched keys survive the read-modify-write.
        assert_eq!(after["tool_rules"]["Bash"]["deny_patterns"][0], "sudo");
        assert_eq!(after["auto_approve_level"], "read");
        // The legacy file is NOT created on this path.
        assert!(!dir.path().join("auto-approve.json").exists());
    }

    #[test]
    fn always_allow_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        persist_auto_approve("Edit", dir.path()).unwrap();
        persist_auto_approve("Edit", dir.path()).unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(after["auto_approve_add"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn always_allow_falls_back_to_legacy_file_when_config_absent() {
        // No config.json → the hook is on the legacy/defaults path, so the
        // legacy file is the one it will actually read.
        let dir = tempfile::tempdir().unwrap();

        persist_auto_approve("Bash", dir.path()).unwrap();

        let legacy: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("auto-approve.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(legacy["auto_approve"][0], "Bash");
        assert!(!dir.path().join("config.json").exists());
    }

    #[test]
    fn always_allow_refuses_wrong_typed_add_list() {
        // A hand-edited `"auto_approve_add": "Bash"` (string, not array) must
        // error, not log success while the hook ignores the "addition".
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, r#"{"auto_approve_add": "Bash"}"#).unwrap();

        persist_auto_approve("Edit", dir.path())
            .expect_err("non-array auto_approve_add must refuse");
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            r#"{"auto_approve_add": "Bash"}"#,
            "refused update must not touch the file"
        );
    }

    #[test]
    fn always_allow_refuses_corrupt_config_json() {
        // itr#308: never rewrite a corrupt config from a lossy fallback.
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{ broken").unwrap();

        persist_auto_approve("Bash", dir.path()).expect_err("corrupt config.json must refuse");
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), "{ broken");
    }

    #[test]
    fn always_allow_refuses_corrupt_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("auto-approve.json");
        std::fs::write(&legacy_path, "not json").unwrap();

        persist_auto_approve("Bash", dir.path())
            .expect_err("corrupt auto-approve.json must refuse");
        assert_eq!(std::fs::read_to_string(&legacy_path).unwrap(), "not json");
    }

    fn ids(items: &[(uuid::Uuid, String)]) -> Vec<uuid::Uuid> {
        items.iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn partition_fresh_device_allows_everything() {
        let bash = (uuid::Uuid::new_v4(), "Bash".to_string());
        let read = (uuid::Uuid::new_v4(), "Read".to_string());
        let (gated, allowed) = partition_sudo_gated(true, vec![bash.clone(), read.clone()]);

        // A fresh reauth grace holds nothing back — even sudo-class tools pass.
        assert!(gated.is_empty());
        assert_eq!(ids(&allowed), vec![bash.0, read.0]);
    }

    #[test]
    fn partition_stale_device_gates_only_sudo_tools() {
        let bash = (uuid::Uuid::new_v4(), "Bash".to_string());
        let edit = (uuid::Uuid::new_v4(), "Edit".to_string());
        let read = (uuid::Uuid::new_v4(), "Read".to_string());
        let (gated, allowed) =
            partition_sudo_gated(false, vec![bash.clone(), read.clone(), edit.clone()]);

        // Stale device: sudo-class (Bash, Edit) held back; read-only (Read) passes.
        assert_eq!(ids(&gated), vec![bash.0, edit.0]);
        assert_eq!(ids(&allowed), vec![read.0]);
    }

    #[test]
    fn partition_empty_input_yields_empty_partitions() {
        let (gated, allowed) = partition_sudo_gated(false, Vec::new());
        assert!(gated.is_empty());
        assert!(allowed.is_empty());
    }

    #[tokio::test]
    async fn conn_channel_is_bounded_and_drops_when_full() {
        // A bounded channel must accept exactly its capacity then reject (drop)
        // excess sends as Full — the policy the terminal forwarders rely on so a
        // stalled TUI can't grow memory without limit (itr#82).
        let (tx, _rx) =
            tokio::sync::mpsc::channel::<wisphive_protocol::ServerMessage>(CONN_CHANNEL_CAPACITY);

        let mut accepted = 0usize;
        let mut rejected_full = 0usize;
        for _ in 0..(CONN_CHANNEL_CAPACITY * 4) {
            match tx.try_send(wisphive_protocol::ServerMessage::Welcome {
                version: wisphive_protocol::PROTOCOL_VERSION,
            }) {
                Ok(()) => accepted += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => rejected_full += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
            }
        }

        assert_eq!(
            accepted, CONN_CHANNEL_CAPACITY,
            "a bounded channel must accept exactly its capacity before backing up"
        );
        assert!(
            rejected_full > 0,
            "excess sends past capacity must be rejected as Full (dropped), not queued"
        );
    }

    // ---- itr#83: capped socket line length ----

    #[tokio::test]
    async fn capped_reader_accepts_normal_line() {
        let payload = b"{\"type\":\"hello\"}\nrest";
        let mut reader = BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let line = read_capped_line(&mut reader, &mut buf).await.unwrap();
        assert_eq!(line.as_deref(), Some("{\"type\":\"hello\"}"));
        // buf is cleared for the next line.
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn capped_reader_strips_crlf() {
        let payload = b"line-one\r\n";
        let mut reader = BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let line = read_capped_line(&mut reader, &mut buf).await.unwrap();
        assert_eq!(line.as_deref(), Some("line-one"));
    }

    #[tokio::test]
    async fn capped_reader_returns_none_on_clean_eof() {
        let payload: &[u8] = b"";
        let mut reader = BufReader::new(payload);
        let mut buf = Vec::new();
        let line = read_capped_line(&mut reader, &mut buf).await.unwrap();
        assert!(line.is_none());
    }

    #[tokio::test]
    async fn capped_reader_rejects_over_limit_line() {
        // A line longer than MAX_LINE_BYTES with no newline must be rejected as
        // an error (bounded memory) rather than buffered until OOM.
        let oversized = vec![b'a'; MAX_LINE_BYTES + 16];
        let mut reader = BufReader::new(&oversized[..]);
        let mut buf = Vec::new();
        let result = read_capped_line(&mut reader, &mut buf).await;
        assert!(
            result.is_err(),
            "an over-limit line must error, not grow unbounded"
        );
        // The accumulator never exceeds the cap.
        assert!(buf.len() <= MAX_LINE_BYTES);
    }

    #[tokio::test]
    async fn capped_reader_accepts_line_at_exactly_the_limit() {
        // A line of exactly MAX_LINE_BYTES followed by a newline is legitimate.
        let mut payload = vec![b'b'; MAX_LINE_BYTES];
        payload.push(b'\n');
        let mut reader = BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let line = read_capped_line(&mut reader, &mut buf).await.unwrap();
        assert_eq!(line.map(|l| l.len()), Some(MAX_LINE_BYTES));
    }

    #[test]
    fn sweep_removes_existing_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for name in ["cc-aaa", "cc-bbb", "codex-ccc"] {
            std::fs::write(sessions.join(name), "").unwrap();
        }
        assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 3);

        sweep_stale_session_markers(&sessions);

        assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 0);
        assert!(
            sessions.exists(),
            "sweep must not remove the directory itself"
        );
    }

    #[test]
    fn sweep_is_noop_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        assert!(!sessions.exists());

        sweep_stale_session_markers(&sessions);

        assert!(!sessions.exists(), "sweep must not create the directory");
    }

    #[test]
    fn sweep_is_noop_when_dir_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        sweep_stale_session_markers(&sessions);

        assert!(sessions.exists());
        assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 0);
    }

    // ── Socket hardening (itr#81) ──────────────────────────────────────────

    #[test]
    fn set_socket_permissions_forces_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("wisphive.sock");

        // Bind a real Unix socket so we chmod an actual socket inode (mirrors
        // what the daemon does after `UnixListener::bind`).
        let _listener = UnixListener::bind(&sock).unwrap();

        set_socket_permissions(&sock).unwrap();

        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        // Mask off the file-type bits; only the permission bits matter.
        assert_eq!(
            mode & 0o777,
            0o600,
            "socket must be owner-only (srw-------), got {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn peer_uid_allowed_accepts_same_uid_rejects_others() {
        // Same uid as the daemon → allowed.
        assert!(peer_uid_allowed(1000, 1000));
        assert!(peer_uid_allowed(0, 0));

        // Any other uid → rejected, including root vs non-root either way.
        assert!(!peer_uid_allowed(1001, 1000));
        assert!(!peer_uid_allowed(0, 1000));
        assert!(!peer_uid_allowed(1000, 0));
        assert!(!peer_uid_allowed(65534, 1000)); // nobody
    }
}
