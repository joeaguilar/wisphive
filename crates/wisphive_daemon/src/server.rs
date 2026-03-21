use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};
use tracing::{error, info, warn};
use wisphive_protocol::{
    ClientMessage, ClientType, Decision, PROTOCOL_VERSION, ServerMessage, encode,
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

        let mut reap_interval = tokio::time::interval(Duration::from_secs(5));
        reap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // Periodically reap exited agent processes
                _ = reap_interval.tick() => {
                    let mut pr = self.process_registry.lock().await;
                    let exited = pr.reap_exited().await;
                    for (agent_id, exit_code) in exited {
                        let _ = self.tui_tx.send(ServerMessage::AgentExited {
                            agent_id,
                            exit_code,
                        });
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
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, queue, process_registry, agent_registry, tui_tx, state_db, timeout).await {
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
                    handle_hook(lines, writer, queue, agent_registry, tui_tx.clone(), state_db, hook_timeout_secs).await
                }
                ClientType::Tui => {
                    handle_tui(lines, writer, queue, process_registry, state_db, tui_tx).await
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

            // Register agent and broadcast to TUI clients
            let agent_info = {
                let mut reg = agent_registry.lock().await;
                reg.register(agent_id.clone(), req.agent_type.clone(), req.project.clone())
            };
            let _ = tui_tx.send(ServerMessage::AgentConnected(agent_info));

            // Persist for crash recovery
            state_db.persist_pending(&req).await?;

            // Send passive notification so user knows to check the TUI
            crate::notify::notify_decision(&req);

            // Enqueue and get receiver
            let rx = {
                let mut q = queue.lock().await;
                q.enqueue(req)
            };

            // Block until TUI responds or timeout
            let decision = match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
                Ok(Ok(decision)) => decision,
                Ok(Err(_)) => {
                    warn!(%id, "decision channel dropped, defaulting to approve");
                    Decision::Approve
                }
                Err(_) => {
                    warn!(%id, "hook timed out after {timeout_secs}s, defaulting to approve");
                    Decision::Approve
                }
            };

            // Log resolution
            state_db.resolve_pending(id, decision).await?;

            // Deregister agent and broadcast to TUI clients
            {
                let mut reg = agent_registry.lock().await;
                reg.deregister(&agent_id);
            }
            let _ = tui_tx.send(ServerMessage::AgentDisconnected { agent_id });

            // Send response to hook
            let resp = encode(&ServerMessage::DecisionResponse { id, decision })?;
            writer.write_all(resp.as_bytes()).await?;
        }
        ClientMessage::ToolResult(result) => {
            // Fire-and-forget: attach result to matching decision_log entry
            match state_db
                .attach_tool_result(&result.agent_id, &result.tool_name, &result.tool_result)
                .await
            {
                Ok(Some(id)) => {
                    info!(%id, tool = %result.tool_name, agent = %result.agent_id, "tool result attached");
                }
                Ok(None) => {
                    warn!(tool = %result.tool_name, agent = %result.agent_id,
                          "tool result received but no matching decision found");
                }
                Err(e) => {
                    warn!("failed to store tool result: {e}");
                }
            }
        }
        _ => {
            let err = encode(&ServerMessage::Error {
                message: "expected DecisionRequest or ToolResult from hook".into(),
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
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
) -> Result<()> {
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
                            ClientMessage::Approve { id } => {
                                let mut q = queue.lock().await;
                                q.resolve(id, Decision::Approve);
                            }
                            ClientMessage::Deny { id } => {
                                let mut q = queue.lock().await;
                                q.resolve(id, Decision::Deny);
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
