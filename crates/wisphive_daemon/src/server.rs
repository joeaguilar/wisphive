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

/// The main daemon server. Listens on a Unix socket and dispatches
/// hook and TUI connections.
pub struct Server {
    config: DaemonConfig,
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
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

        Ok(Self {
            config,
            queue,
            process_registry,
            agent_registry,
            tui_tx,
            state_db,
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
                            let queue = self.queue.clone();
                            let process_registry = self.process_registry.clone();
                            let agent_registry = self.agent_registry.clone();
                            let tui_tx = self.tui_tx.clone();
                            let state_db = self.state_db.clone();
                            let timeout = self.config.hook_timeout_secs;
                            let notifications = self.config.notifications_enabled;
                            let home_dir = self.config.home_dir.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, queue, process_registry, agent_registry, tui_tx, state_db, timeout, notifications, home_dir).await {
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

        // Cleanup socket
        let _ = std::fs::remove_file(&self.config.socket_path);
        info!("server stopped");
        Ok(())
    }
}

/// Handle a single client connection. Dispatches based on the Hello handshake.
async fn handle_connection(
    stream: UnixStream,
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
    hook_timeout_secs: u64,
    notifications_enabled: bool,
    home_dir: PathBuf,
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
                    handle_hook(lines, writer, queue, agent_registry, tui_tx.clone(), state_db, hook_timeout_secs, notifications_enabled).await
                }
                ClientType::Tui => {
                    handle_tui(lines, writer, queue, process_registry, agent_registry, state_db, tui_tx, home_dir).await
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
async fn handle_hook(
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut writer: tokio::net::unix::OwnedWriteHalf,
    queue: Arc<Mutex<DecisionQueue>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    tui_tx: broadcast::Sender<ServerMessage>,
    state_db: Arc<StateDb>,
    timeout_secs: u64,
    notifications_enabled: bool,
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
                let mut reg = agent_registry.lock().await;
                reg.register(agent_id.clone(), req.agent_type.clone(), req.project.clone())
            };
            if is_new {
                let _ = tui_tx.send(ServerMessage::AgentConnected(agent_info));
            }

            // Persist for crash recovery
            state_db.persist_pending(&req).await?;

            // Send passive notification so user knows to check the TUI
            if notifications_enabled {
                crate::notify::notify_decision(&req);
            }

            // Enqueue and get receiver
            let rx = {
                let mut q = queue.lock().await;
                q.enqueue(req)
            };

            // Block until TUI responds or timeout
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
            if rich.always_allow {
                if let Err(e) = persist_auto_approve(&req_tool_name, &config_home) {
                    warn!("failed to persist auto-approve: {e}");
                }
            }

            // Log resolution (skip audit log for Ask/defer decisions)
            if rich.decision != Decision::Ask {
                state_db.resolve_pending(id, rich.decision).await?;
            }

            // Touch last_seen (agent stays registered, reaped on inactivity)
            {
                let mut reg = agent_registry.lock().await;
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
                let mut reg = agent_registry.lock().await;
                reg.touch(&result.agent_id);
            }
            // Fire-and-forget: attach result to matching decision_log entry
            match state_db
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
                let mut reg = agent_registry.lock().await;
                reg.register(agent_id, agent_type, project)
            };
            if is_new {
                let _ = tui_tx.send(ServerMessage::AgentConnected(info));
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
    queue: Arc<Mutex<DecisionQueue>>,
    process_registry: Arc<Mutex<ProcessRegistry>>,
    agent_registry: Arc<Mutex<AgentRegistry>>,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
    home_dir: PathBuf,
) -> Result<()> {
    // Send agents snapshot
    let agents_snap = {
        let reg = agent_registry.lock().await;
        reg.snapshot()
    };
    let agents_msg = encode(&ServerMessage::AgentsSnapshot { agents: agents_snap })?;
    writer.write_all(agents_msg.as_bytes()).await?;

    // Send initial queue snapshot
    let snapshot = {
        let q = queue.lock().await;
        q.snapshot()
    };
    let snap_msg = encode(&ServerMessage::QueueSnapshot { items: snapshot })?;
    writer.write_all(snap_msg.as_bytes()).await?;

    // Subscribe to broadcast events for this TUI
    let mut tui_rx = tui_tx.subscribe();

    loop {
        tokio::select! {
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
                                let mut q = queue.lock().await;
                                q.resolve(id, rich);
                            }
                            ClientMessage::Deny { id, message } => {
                                let rich = RichDecision {
                                    decision: Decision::Deny,
                                    message,
                                    ..RichDecision::deny()
                                };
                                let mut q = queue.lock().await;
                                q.resolve(id, rich);
                            }
                            ClientMessage::Ask { id } => {
                                let mut q = queue.lock().await;
                                q.resolve(id, RichDecision::from(Decision::Ask));
                            }
                            ClientMessage::ApproveAll { ref filter } => {
                                let mut q = queue.lock().await;
                                let n = q.resolve_all(filter, Decision::Approve);
                                info!("approved {n} decisions");
                            }
                            ClientMessage::DenyAll { ref filter } => {
                                let mut q = queue.lock().await;
                                let n = q.resolve_all(filter, Decision::Deny);
                                info!("denied {n} decisions");
                            }
                            ClientMessage::SpawnAgent(req) => {
                                let mut pr = process_registry.lock().await;
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
                                let pr = process_registry.lock().await;
                                let agents = pr.list();
                                let resp = encode(&ServerMessage::AgentList { agents })?;
                                writer.write_all(resp.as_bytes()).await?;
                            }
                            ClientMessage::ReimportEvents => {
                                let events_path = home_dir.join("events.jsonl");
                                match crate::event_ingest::reimport_all(&events_path, &state_db).await {
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
                            ClientMessage::QueryHistory { ref agent_id, limit } => {
                                let limit = limit.unwrap_or(200);
                                match state_db.query_history(agent_id.as_deref(), limit).await {
                                    Ok(entries) => {
                                        let resp = encode(&ServerMessage::HistoryResponse { entries })?;
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
                                match state_db.search_history(search).await {
                                    Ok(entries) => {
                                        let resp = encode(&ServerMessage::HistoryResponse { entries })?;
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
                                match state_db.query_sessions().await {
                                    Ok(mut sessions) => {
                                        // Enrich with live status
                                        let live_agents = {
                                            let reg = agent_registry.lock().await;
                                            reg.snapshot()
                                        };
                                        let live_ids: std::collections::HashSet<String> =
                                            live_agents.iter().map(|a| a.agent_id.clone()).collect();

                                        // Pending counts from queue
                                        let pending_counts: std::collections::HashMap<String, u32> = {
                                            let q = queue.lock().await;
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
                                                    first_seen: agent.started_at,
                                                    last_seen: agent.started_at,
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
                                match state_db.query_projects().await {
                                    Ok(mut projects) => {
                                        // Enrich with live agent presence
                                        let live_agents = {
                                            let reg = agent_registry.lock().await;
                                            reg.snapshot()
                                        };
                                        let mut live_projects: std::collections::HashSet<std::path::PathBuf> =
                                            std::collections::HashSet::new();
                                        for agent in &live_agents {
                                            live_projects.insert(agent.project.clone());
                                        }

                                        // Pending counts per project
                                        let pending_counts: std::collections::HashMap<std::path::PathBuf, u32> = {
                                            let q = queue.lock().await;
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
                                                    first_seen: agent.started_at,
                                                    last_seen: agent.started_at,
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
                                let mut pr = process_registry.lock().await;
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
                            ClientMessage::ApprovePermission { id, suggestion_index, message } => {
                                // Look up the selected suggestion from the queued request
                                let selected = {
                                    let q = queue.lock().await;
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
                                let mut q = queue.lock().await;
                                q.resolve(id, rich);
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
