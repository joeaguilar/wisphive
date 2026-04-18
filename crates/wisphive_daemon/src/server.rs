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
use crate::terminal::TerminalSessionManager;

/// Shared context passed to each connection handler, replacing many individual arguments.
struct ConnectionContext {
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
    terminal_manager: Arc<TerminalSessionManager>,
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
        let terminal_manager =
            Arc::new(TerminalSessionManager::new(state_db.clone(), tui_tx.clone()));

        Ok(Self {
            config,
            queue,
            process_registry,
            agent_registry,
            tui_tx,
            state_db,
            terminal_manager,
        })
    }

    /// Start listening for connections. Runs until shutdown signal.
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
        // Clean up stale socket
        let _ = std::fs::remove_file(&self.config.socket_path);

        let listener = UnixListener::bind(&self.config.socket_path)?;
        info!(path = %self.config.socket_path.display(), "listening");

        // Spawn event ingest task (tails events.jsonl → decision_log)
        let events_path = self.config.home_dir.join("events.jsonl");
        let _ingest_handle = crate::event_ingest::spawn_event_ingest(
            events_path,
            self.state_db.clone(),
        );

        let mut reap_interval = tokio::time::interval(Duration::from_secs(5));
        reap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Run retention on startup
        let archive_path = self.config.log_dir.join("decision_log.jsonl");
        {
            match self.state_db.archive_and_prune(
                &archive_path,
                self.config.retention_max_rows,
                self.config.retention_max_age_days,
            ).await {
                Ok(0) => {}
                Ok(n) => info!(n, "startup retention: archived entries"),
                Err(e) => warn!("startup retention failed: {e}"),
            }
        }

        let mut retention_interval = tokio::time::interval(Duration::from_secs(3600));
        retention_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        retention_interval.tick().await; // skip immediate tick (we just ran on startup)

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
                // Periodic retention: archive + prune old decision_log entries
                _ = retention_interval.tick() => {
                    match self.state_db.archive_and_prune(
                        &archive_path,
                        self.config.retention_max_rows,
                        self.config.retention_max_age_days,
                    ).await {
                        Ok(0) => {}
                        Ok(n) => info!(n, "retention: archived entries"),
                        Err(e) => warn!("retention failed: {e}"),
                    }
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
async fn handle_connection(
    stream: UnixStream,
    ctx: &ConnectionContext,
) -> Result<()> {
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
                ClientType::Hook => {
                    handle_hook(lines, writer, ctx).await
                }
                ClientType::Tui => {
                    handle_tui(lines, writer, ctx).await
                }
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
                reg.register(agent_id.clone(), req.agent_type.clone(), req.project.clone())
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
                && let Err(e) = persist_auto_approve(&req_tool_name, &config_home) {
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
            match ctx.state_db
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
        ClientMessage::AgentRegister { agent_id, agent_type, project } => {
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
    let agents_msg = encode(&ServerMessage::AgentsSnapshot { agents: agents_snap })?;
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
                        let msg: ClientMessage = match wisphive_protocol::decode(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("invalid TUI message: {e}");
                                continue;
                            }
                        };
                        match msg {
                            ClientMessage::Approve { id, message, updated_input, always_allow, additional_context } => {
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
                                let mut q = ctx.queue.lock().await;
                                q.resolve(id, RichDecision::from(Decision::Ask));
                                // Ask/defer decisions are not persisted to the audit log
                            }
                            ClientMessage::ApproveAll { ref filter } => {
                                let ids = {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve_all(filter, Decision::Approve)
                                };
                                info!("approved {} decisions", ids.len());
                                for id in ids {
                                    if let Err(e) = ctx.state_db.resolve_pending(id, Decision::Approve).await {
                                        warn!("eager persist failed for {id}: {e}");
                                    }
                                }
                            }
                            ClientMessage::DenyAll { ref filter } => {
                                let ids = {
                                    let mut q = ctx.queue.lock().await;
                                    q.resolve_all(filter, Decision::Deny)
                                };
                                info!("denied {} decisions", ids.len());
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
                            ClientMessage::ApprovePermission { id, suggestion_index, message } => {
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
