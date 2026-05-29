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

/// Handle a single client connection. Dispatches based on the Hello handshake.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(stream: UnixStream, ctx: &ConnectionContext) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Read the Hello handshake
    let first_line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before hello"))?;

    let hello: ClientMessage = wisphive_protocol::decode(&first_line)?;

    match hello {
        ClientMessage::Hello { client, version } => {
            if version != PROTOCOL_VERSION {
                let err = encode(&ServerMessage::Error {
                    message: format!("unsupported protocol version: {version}"),
                })?;
                writer.write_all(err.as_bytes()).await?;
                return Ok(());
            }

            let welcome = encode(&ServerMessage::Welcome {
                version: PROTOCOL_VERSION,
            })?;
            writer.write_all(welcome.as_bytes()).await?;

            match client {
                ClientType::Hook => handle_hook(lines, writer, ctx).await,
                ClientType::Tui => handle_tui(lines, writer, ctx).await,
            }
        }
        _ => {
            let err = encode(&ServerMessage::Error {
                message: "expected Hello as first message".into(),
            })?;
            writer.write_all(err.as_bytes()).await?;
            Ok(())
        }
    }
}

/// Handle a hook connection: receive DecisionRequest, block until resolved.
#[allow(clippy::too_many_arguments)]
async fn handle_hook(
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
) -> Result<()> {
    let line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow::anyhow!("hook disconnected before sending request"))?;

    let msg: ClientMessage = wisphive_protocol::decode(&line)?;

    match msg {
        ClientMessage::DecisionRequest(req) => {
            let id = req.id;
            let agent_id = req.agent_id.clone();
            let req_tool_name = req.tool_name.clone();
            let config_home = std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                .join(".wisphive");

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

            // Persist for crash recovery
            ctx.state_db.persist_pending(&req).await?;

            // Send passive notification so user knows to check the TUI
            if ctx.notifications_enabled {
                crate::notify::notify_decision(&req);
            }

            // Enqueue and get receiver
            let rx = {
                let mut q = ctx.queue.lock().await;
                q.enqueue(req)
            };

            // Block until TUI responds or timeout
            let timeout_secs = ctx.hook_timeout_secs;
            let rich = match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
                Ok(Ok(rich)) => rich,
                Ok(Err(_)) => {
                    warn!(%id, "decision channel dropped, defaulting to approve");
                    RichDecision::approve()
                }
                Err(_) => {
                    warn!(%id, "hook timed out after {timeout_secs}s, defaulting to approve");
                    RichDecision::approve()
                }
            };

            // Persist auto-approve if requested
            if rich.always_allow
                && let Err(e) = persist_auto_approve(&req_tool_name, &config_home)
            {
                warn!("failed to persist auto-approve: {e}");
            }

            // Log resolution (skip audit log for Ask/defer decisions)
            if rich.decision != Decision::Ask {
                ctx.state_db.resolve_pending(id, rich.decision).await?;
            }

            // Touch last_seen (agent stays registered, reaped on inactivity)
            {
                let mut reg = ctx.agent_registry.lock().await;
                reg.touch(&agent_id);
            }

            // Send rich response to hook
            let resp = encode(&ServerMessage::DecisionResponse {
                id,
                decision: rich.decision,
                message: rich.message,
                updated_input: rich.updated_input,
                additional_context: rich.additional_context,
                selected_permission: rich.selected_permission,
            })?;
            writer.write_all(resp.as_bytes()).await?;
        }
        ClientMessage::ToolResult(result) => {
            // Touch last_seen for the agent
            {
                let mut reg = ctx.agent_registry.lock().await;
                reg.touch(&result.agent_id);
            }
            // Fire-and-forget: attach result to matching decision_log entry
            match ctx
                .state_db
                .attach_tool_result(
                    &result.agent_id,
                    &result.tool_name,
                    &result.tool_result,
                    result.tool_use_id.as_deref(),
                )
                .await
            {
                Ok(Some(id)) => {
                    info!(%id, tool = %result.tool_name, agent = %result.agent_id, "tool result attached");
                }
                Ok(None) => {
                    // Auto-approved events may still be in the JSONL ingest pipeline
                    debug!(tool = %result.tool_name, agent = %result.agent_id,
                          "tool result: no matching decision yet (may be pending ingest)");
                }
                Err(e) => {
                    warn!("failed to store tool result: {e}");
                }
            }
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
            let err = encode(&ServerMessage::Error {
                message: "expected DecisionRequest, ToolResult, or AgentRegister from hook".into(),
            })?;
            writer.write_all(err.as_bytes()).await?;
        }
    }

    Ok(())
}

/// Handle a TUI connection: stream events, receive commands.
async fn handle_tui(
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    ctx: &ConnectionContext,
) -> Result<()> {
    use tokio::sync::mpsc;

    // Send agents snapshot
    let agents_snap = {
        let reg = ctx.agent_registry.lock().await;
        reg.snapshot()
    };
    let agents_msg = encode(&ServerMessage::AgentsSnapshot {
        agents: agents_snap,
    })?;
    writer.write_all(agents_msg.as_bytes()).await?;

    // Send initial queue snapshot
    let snapshot = {
        let q = ctx.queue.lock().await;
        q.snapshot()
    };
    let snap_msg = encode(&ServerMessage::QueueSnapshot { items: snapshot })?;
    writer.write_all(snap_msg.as_bytes()).await?;

    // Subscribe to broadcast events for this TUI
    let mut tui_rx = ctx.tui_tx.subscribe();

    // Per-connection channel for messages produced by worker tasks
    // (e.g. per-session terminal forwarders). The select loop drains this
    // and writes to the single owned socket, so there's no lock contention
    // on the writer.
    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ServerMessage>();

    // Attached terminal sessions on this connection. Aborted on detach,
    // disconnect, or TermEnded. Key: terminal session id.
    let mut term_attachments: std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    loop {
        tokio::select! {
            // Per-connection messages from worker tasks (e.g. terminal forwarders)
            msg = conn_rx.recv() => {
                match msg {
                    Some(m) => {
                        let encoded = encode(&m)?;
                        if writer.write_all(encoded.as_bytes()).await.is_err() {
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
                        let encoded = encode(&msg)?;
                        if writer.write_all(encoded.as_bytes()).await.is_err() {
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
            line = lines.next_line() => {
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
                        // Every decision arm below logs `%device_id` (None = local
                        // TUI, implicitly trusted). The Approve / ApproveAll arms
                        // consult `ctx.reauth` before honouring web-origin
                        // approvals of sudo-class tools (itr#218).
                        let device_id = command.device_id.clone();
                        let msg = command.body;
                        match msg {
                            ClientMessage::Approve { id, message, updated_input, always_allow, additional_context } => {
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
                                        let encoded = encode(&reauth_msg)?;
                                        writer.write_all(encoded.as_bytes()).await?;
                                        debug!(%id, tool = %tool_name, device_id = %dev.0, "sudo gate: reauth required");
                                        continue;
                                    }
                                }

                                let rich = RichDecision {
                                    decision: Decision::Approve,
                                    message,
                                    updated_input,
                                    always_allow,
                                    additional_context,
                                    selected_permission: None,
                                };
                                {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve(id, rich);
                                }
                                // Eagerly persist so subsequent history queries see this decision.
                                // The hook handler's resolve_pending is idempotent (no-op if already done).
                                if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Approve).await {
                                    warn!("eager persist failed for {id}: {e}");
                                }
                            }
                            ClientMessage::Deny { id, message } => {
                                info!(?device_id, %id, "deny");
                                let rich = RichDecision {
                                    decision: Decision::Deny,
                                    message,
                                    ..RichDecision::deny()
                                };
                                {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve(id, rich);
                                }
                                if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Deny).await {
                                    warn!("eager persist failed for {id}: {e}");
                                }
                            }
                            ClientMessage::Ask { id } => {
                                info!(?device_id, %id, "ask");
                                let mut q = ctx.queue.lock().await;
                                q.resolve(id, RichDecision::from(Decision::Ask));
                                // Ask/defer decisions are not persisted to the audit log
                            }
                            ClientMessage::ApproveAll { ref filter } => {
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
                                    let (gated, allowed): (Vec<_>, Vec<_>) = if fresh {
                                        (Vec::new(), matching)
                                    } else {
                                        matching.into_iter().partition(|(_, t)| crate::sudo_gate::is_sudo_tool(t))
                                    };

                                    let allowed_ids: Vec<uuid::Uuid> = {
                                        let mut q = ctx.queue.lock().await;
                                        allowed
                                            .iter()
                                            .filter(|(id, _)| q.resolve(*id, RichDecision::from(Decision::Approve)))
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
                                        if let Err(e) = ctx.state_db.resolve_pending(*id, Decision::Approve).await {
                                            warn!("eager persist failed for {id}: {e}");
                                        }
                                    }
                                    for (id, tool_name) in gated {
                                        let reauth_msg = ServerMessage::WebReauthRequired {
                                            device_id: dev.0.clone(),
                                            request_id: id.to_string(),
                                            tool_name: tool_name.clone(),
                                            at: chrono::Utc::now(),
                                        };
                                        let encoded = encode(&reauth_msg)?;
                                        writer.write_all(encoded.as_bytes()).await?;
                                        debug!(%id, tool = %tool_name, device_id = %dev.0, "sudo gate: reauth required (approve_all)");
                                    }
                                } else {
                                    let ids = {
                                        let mut q = ctx.queue.lock().await;
                                        q.resolve_all(filter, Decision::Approve)
                                    };
                                    info!(?device_id, count = ids.len(), "approve_all");
                                    for id in ids {
                                        if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Approve).await {
                                            warn!("eager persist failed for {id}: {e}");
                                        }
                                    }
                                }
                            }
                            ClientMessage::DenyAll { ref filter } => {
                                let ids = {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve_all(filter, Decision::Deny)
                                };
                                info!(?device_id, count = ids.len(), "deny_all");
                                for id in ids {
                                    if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Deny).await {
                                        warn!("eager persist failed for {id}: {e}");
                                    }
                                }
                            }
                            ClientMessage::SpawnAgent(req) => {
                                let mut pr = ctx.process_registry.lock().await;
                                match pr.spawn_agent(req).await {
                                    Ok(agent) => {
                                        let resp = encode(&ServerMessage::AgentSpawned(agent))?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("failed to spawn agent: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::ListAgents => {
                                let pr = ctx.process_registry.lock().await;
                                let agents = pr.list();
                                let resp = encode(&ServerMessage::AgentList { agents })?;
                                writer.write_all(resp.as_bytes()).await?;
                            }
                            ClientMessage::ReimportEvents => {
                                let events_path = ctx.home_dir.join("events.jsonl");
                                match crate::event_ingest::reimport_all(&events_path, &ctx.state_db).await {
                                    Ok(count) => {
                                        let resp = encode(&ServerMessage::ReimportComplete { count })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("reimport failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::QueryHistory { ref agent_id, limit, ref request_id } => {
                                let limit = limit.unwrap_or(200);
                                match ctx.state_db.query_history(agent_id.as_deref(), limit).await {
                                    Ok(entries) => {
                                        let resp = encode(&ServerMessage::HistoryResponse { entries, request_id: request_id.clone() })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("history query failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::SearchHistory(ref search) => {
                                match ctx.state_db.search_history(search).await {
                                    Ok(entries) => {
                                        let resp = encode(&ServerMessage::HistoryResponse { entries, request_id: search.request_id.clone() })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("search failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
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
                                            session.pending_count = pending_counts.get(&session.agent_id).copied().unwrap_or(0);
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
                                                    pending_count: pending_counts.get(&agent.agent_id).copied().unwrap_or(0),
                                                });
                                            }
                                        }

                                        // Sort: live+pending first, then live, then by last_seen DESC
                                        sessions.sort_by(|a, b| {
                                            let a_key = (a.is_live && a.pending_count > 0, a.is_live, a.last_seen);
                                            let b_key = (b.is_live && b.pending_count > 0, b.is_live, b.last_seen);
                                            b_key.partial_cmp(&a_key).unwrap_or(std::cmp::Ordering::Equal)
                                        });

                                        let resp = encode(&ServerMessage::SessionsResponse { sessions })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("sessions query failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
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
                                            project.pending_count = pending_counts.get(&project.project).copied().unwrap_or(0);
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
                                                    pending_count: pending_counts.get(&agent.project).copied().unwrap_or(0),
                                                    has_live_agents: true,
                                                });
                                            }
                                        }

                                        projects.sort_by(|a, b| {
                                            let a_key = (a.has_live_agents && a.pending_count > 0, a.has_live_agents, a.last_seen);
                                            let b_key = (b.has_live_agents && b.pending_count > 0, b.has_live_agents, b.last_seen);
                                            b_key.partial_cmp(&a_key).unwrap_or(std::cmp::Ordering::Equal)
                                        });

                                        let resp = encode(&ServerMessage::ProjectsResponse { projects })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("projects query failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::StopAgent { ref agent_id } => {
                                let mut pr = ctx.process_registry.lock().await;
                                match pr.stop_agent(agent_id).await {
                                    Ok(exit_code) => {
                                        let resp = encode(&ServerMessage::AgentExited {
                                            agent_id: agent_id.clone(),
                                            exit_code,
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::Error {
                                            message: format!("{e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermCreate { label, command, args, cwd, cols, rows, env } => {
                                match ctx.terminal_manager
                                    .create(label, command, args, cwd, cols, rows, env)
                                    .await
                                {
                                    Ok(meta) => {
                                        let resp = encode(&ServerMessage::TermCreated(meta))?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::TermError {
                                            id: None,
                                            message: format!("term create failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
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
                                        let encoded = encode(&catchup)?;
                                        writer.write_all(encoded.as_bytes()).await?;

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
                                                        if tx.send(msg).is_err() {
                                                            break;
                                                        }
                                                    }
                                                    Err(broadcast::error::RecvError::Lagged(_)) => {
                                                        let _ = tx.send(ServerMessage::TermError {
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
                                        let resp = encode(&ServerMessage::TermError {
                                            id: Some(id),
                                            message: "terminal session not found or no longer running".into(),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermDetach { id } => {
                                if let Some(handle) = term_attachments.remove(&id) {
                                    handle.abort();
                                }
                            }
                            ClientMessage::TermInput { id, data } => {
                                match crate::terminal::decode_b64(&data) {
                                    Ok(bytes) => {
                                        if let Err(e) = ctx.terminal_manager.write_input(id, bytes).await {
                                            let resp = encode(&ServerMessage::TermError {
                                                id: Some(id),
                                                message: format!("term input failed: {e}"),
                                            })?;
                                            writer.write_all(resp.as_bytes()).await?;
                                        }
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::TermError {
                                            id: Some(id),
                                            message: format!("invalid term input payload: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermResize { id, cols, rows } => {
                                if let Err(e) = ctx.terminal_manager.resize(id, cols, rows).await {
                                    let resp = encode(&ServerMessage::TermError {
                                        id: Some(id),
                                        message: format!("term resize failed: {e}"),
                                    })?;
                                    writer.write_all(resp.as_bytes()).await?;
                                }
                            }
                            ClientMessage::TermClose { id, kill } => {
                                if let Some(handle) = term_attachments.remove(&id) {
                                    handle.abort();
                                }
                                if let Err(e) = ctx.terminal_manager.close(id, kill).await {
                                    let resp = encode(&ServerMessage::TermError {
                                        id: Some(id),
                                        message: format!("term close failed: {e}"),
                                    })?;
                                    writer.write_all(resp.as_bytes()).await?;
                                }
                            }
                            ClientMessage::TermList => {
                                match ctx.terminal_manager.list_all().await {
                                    Ok(sessions) => {
                                        let resp = encode(&ServerMessage::TermListResponse { sessions })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::TermError {
                                            id: None,
                                            message: format!("term list failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermSetGroup { id, group } => {
                                match ctx.terminal_manager.set_group(id, group.as_deref()).await {
                                    Ok(()) => {
                                        if let Ok(sessions) = ctx.terminal_manager.list_all().await {
                                            let _ = ctx.tui_tx.send(ServerMessage::TermListResponse { sessions });
                                        }
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::TermError {
                                            id: Some(id),
                                            message: format!("term set group failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermReorder { id, sort_order } => {
                                match ctx.terminal_manager.set_sort_order(id, sort_order).await {
                                    Ok(()) => {
                                        if let Ok(sessions) = ctx.terminal_manager.list_all().await {
                                            let _ = ctx.tui_tx.send(ServerMessage::TermListResponse { sessions });
                                        }
                                    }
                                    Err(e) => {
                                        let resp = encode(&ServerMessage::TermError {
                                            id: Some(id),
                                            message: format!("term reorder failed: {e}"),
                                        })?;
                                        writer.write_all(resp.as_bytes()).await?;
                                    }
                                }
                            }
                            ClientMessage::TermReplay { id, from_seq, speed: _ } => {
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
                                                if tx.send(msg).is_err() {
                                                    return;
                                                }
                                            }
                                            let _ = tx.send(ServerMessage::TermReplayDone {
                                                id,
                                                total_events: total,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = tx.send(ServerMessage::TermError {
                                                id: Some(id),
                                                message: format!("replay failed: {e}"),
                                            });
                                        }
                                    }
                                });
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
                                    let ack = encode(&ServerMessage::MarkDeviceFreshAck {
                                        device_id: dev.0.clone(),
                                    })?;
                                    writer.write_all(ack.as_bytes()).await?;
                                } else {
                                    warn!("mark_device_fresh without device_id; ignoring");
                                }
                            }
                            ClientMessage::ApprovePermission { id, suggestion_index, message } => {
                                info!(?device_id, %id, suggestion_index, "approve_permission");
                                // Look up the selected suggestion from the queued request
                                let selected = {
                                    let q = ctx.queue.lock().await;
                                    q.snapshot().iter()
                                        .find(|r| r.id == id)
                                        .and_then(|r| r.permission_suggestions.as_ref())
                                        .and_then(|s| s.get(suggestion_index))
                                        .cloned()
                                };
                                let rich = RichDecision {
                                    decision: Decision::Approve,
                                    message,
                                    updated_input: None,
                                    always_allow: false,
                                    additional_context: None,
                                    selected_permission: selected,
                                };
                                {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve(id, rich);
                                }
                                if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Approve).await {
                                    warn!("eager persist failed for {id}: {e}");
                                }
                            }
                            _ => {
                                warn!("unexpected message from TUI: {:?}", msg);
                            }
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

/// Add a tool to the auto-approve list in ~/.wisphive/auto-approve.json.
fn persist_auto_approve(tool_name: &str, wisphive_dir: &std::path::Path) -> Result<()> {
    let path = wisphive_dir.join("auto-approve.json");
    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
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
        info!(tool = tool_name, "added to auto-approve list");
    }

    std::fs::write(&path, serde_json::to_string_pretty(&config)?)?;
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

#[cfg(test)]
mod tests {
    use super::sweep_stale_session_markers;

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
}
