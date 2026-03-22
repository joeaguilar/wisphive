use std::time::Duration;

use anyhow::Result;
use crossterm::event;
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use wisphive_daemon::DaemonConfig;
use wisphive_protocol::{ClientMessage, ServerMessage};
use wisphive_tui::app::App;
use wisphive_tui::connection::DaemonConnection;
use wisphive_tui::input::{self, InputAction};
use wisphive_tui::ui;

const HISTORY_PAGE_SIZE: u32 = 50;

/// Run the TUI client.
pub async fn run() -> Result<()> {
    let config = DaemonConfig::default_location();
    let log_path = config.log_dir.join("tui.log");
    std::fs::create_dir_all(&config.log_dir)?;

    // File logger for TUI debugging (doesn't interfere with terminal rendering)
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .with_target(true)
        .init();

    tracing::info!("TUI starting, connecting to {:?}", config.socket_path);

    // Connect to daemon
    let mut conn = match DaemonConnection::connect(&config.socket_path).await {
        Ok(conn) => {
            tracing::info!("connected to daemon");
            conn
        }
        Err(e) => {
            tracing::error!("failed to connect: {e}");
            if e.downcast_ref::<std::io::Error>().is_some() {
                anyhow::bail!(
                    "could not connect to daemon. Is it running?\n\n  \
                     Start it with:  wisphive daemon start\n  \
                     Check status:   wisphive doctor\n  \
                     TUI log:        {}",
                    log_path.display()
                );
            }
            return Err(e);
        }
    };

    let mut app = App::new();
    app.connected = true;

    tracing::info!("setting up terminal");

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    tracing::info!("entering main loop");
    let result = run_loop(&mut terminal, &mut app, &mut conn).await;

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if let Err(ref e) = result {
        tracing::error!("TUI exited with error: {e}");
    } else {
        tracing::info!("TUI exited cleanly");
    }

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    conn: &mut DaemonConnection,
) -> Result<()> {
    loop {
        // Draw
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events (terminal input or daemon messages)
        tokio::select! {
            // Check for terminal input
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if event::poll(Duration::from_millis(0))? {
                    let ev = event::read()?;
                    let action = input::handle_event(app, ev);
                    match action {
                        InputAction::Quit => break,
                        InputAction::Approve(id) => {
                            // Track stopped agents before removing from queue
                            if let Some(req) = app.queue.iter().find(|r| r.id == id) {
                                if matches!(req.hook_event_name,
                                    wisphive_protocol::HookEventType::Stop
                                    | wisphive_protocol::HookEventType::SubagentStop
                                ) {
                                    app.stopped_agents.insert(req.agent_id.clone());
                                }
                            }
                            tracing::info!(%id, "approved");
                            conn.send(&ClientMessage::Approve {
                                id,
                                message: None,
                                updated_input: None,
                                always_allow: false,
                                additional_context: None,
                            }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::Deny(id) => {
                            tracing::info!(%id, "denied");
                            conn.send(&ClientMessage::Deny { id, message: None }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::ApproveAll => {
                            tracing::info!("approved all");
                            conn.send(&ClientMessage::ApproveAll { filter: None }).await?;
                            app.queue.clear();
                            app.queue_index = 0;
                            app.rebuild_projects();
                        }
                        InputAction::DenyAll => {
                            tracing::info!("denied all");
                            conn.send(&ClientMessage::DenyAll { filter: None }).await?;
                            app.queue.clear();
                            app.queue_index = 0;
                            app.rebuild_projects();
                        }
                        InputAction::SpawnAgent(req) => {
                            tracing::info!(project = ?req.project, "spawning agent");
                            conn.send(&ClientMessage::SpawnAgent(req)).await?;
                        }
                        InputAction::QueryHistory { agent_id } => {
                            tracing::info!(?agent_id, "querying history");
                            app.history_page = 0;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id,
                                limit: Some(HISTORY_PAGE_SIZE + 1),
                            }).await?;
                        }
                        InputAction::QueryHistoryPage { agent_id, page } => {
                            tracing::info!(?agent_id, page, "querying history page");
                            let offset = page as u32 * HISTORY_PAGE_SIZE;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id,
                                limit: Some(offset + HISTORY_PAGE_SIZE + 1),
                            }).await?;
                        }
                        InputAction::SearchHistory { search } => {
                            tracing::info!(?search.query, "searching history");
                            conn.send(&ClientMessage::SearchHistory(search)).await?;
                        }
                        InputAction::QuerySessions => {
                            tracing::info!("querying sessions");
                            conn.send(&ClientMessage::QuerySessions).await?;
                        }
                        InputAction::QueryProjects => {
                            tracing::info!("querying projects");
                            conn.send(&ClientMessage::QueryProjects).await?;
                        }
                        InputAction::QuerySessionTimeline { agent_id } => {
                            tracing::info!(%agent_id, "querying session timeline");
                            app.session_timeline_page = 0;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id: Some(agent_id),
                                limit: Some(HISTORY_PAGE_SIZE + 1),
                            }).await?;
                        }
                        InputAction::QuerySessionTimelinePage { agent_id, page } => {
                            tracing::info!(%agent_id, page, "querying session timeline page");
                            let offset = page as u32 * HISTORY_PAGE_SIZE;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id: Some(agent_id),
                                limit: Some(offset + HISTORY_PAGE_SIZE + 1),
                            }).await?;
                        }
                        InputAction::DenyWithMessage { id, message } => {
                            tracing::info!(%id, %message, "denied with message");
                            conn.send(&ClientMessage::Deny { id, message: Some(message) }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::AlwaysAllow(id) => {
                            tracing::info!(%id, "always allow");
                            conn.send(&ClientMessage::Approve {
                                id,
                                message: None,
                                updated_input: None,
                                always_allow: true,
                                additional_context: None,
                            }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::ApproveWithInput { id, updated_input } => {
                            tracing::info!(%id, "approve with modified input");
                            conn.send(&ClientMessage::Approve {
                                id,
                                message: None,
                                updated_input: Some(updated_input),
                                always_allow: false,
                                additional_context: None,
                            }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::ApproveWithContext { id, context } => {
                            tracing::info!(%id, "approve with context");
                            conn.send(&ClientMessage::Approve {
                                id,
                                message: None,
                                updated_input: None,
                                always_allow: false,
                                additional_context: Some(context),
                            }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::AskDefer(id) => {
                            tracing::info!(%id, "defer to native prompt");
                            conn.send(&ClientMessage::Ask { id }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::ApprovePermission { id, suggestion_index } => {
                            tracing::info!(%id, suggestion_index, "approve permission");
                            conn.send(&ClientMessage::ApprovePermission {
                                id,
                                suggestion_index,
                                message: None,
                            }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::None => {}
                    }
                }
            }

            // Check for daemon messages
            msg = conn.recv() => {
                match msg? {
                    Some(ServerMessage::AgentsSnapshot { agents }) => {
                        tracing::info!(count = agents.len(), "received agents snapshot");
                        app.agents = agents;
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::QueueSnapshot { ref items }) => {
                        tracing::info!(count = items.len(), "received queue snapshot");
                        app.queue = items.clone();
                        app.queue_index = 0;
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::NewDecision(req)) => {
                        tracing::info!(id = %req.id, tool = %req.tool_name, agent = %req.agent_id, "new decision");
                        app.queue.push(req);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::DecisionResolved { id, .. }) => {
                        tracing::info!(%id, "decision resolved");
                        app.remove_decision(id);
                    }
                    Some(ServerMessage::AgentConnected(info)) => {
                        tracing::info!(agent = %info.agent_id, "agent connected");
                        app.agents.push(info);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::AgentDisconnected { ref agent_id }) => {
                        tracing::info!(agent = %agent_id, "agent disconnected");
                        app.agents.retain(|a| a.agent_id != *agent_id);
                        app.stopped_agents.remove(agent_id);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::AgentExited { ref agent_id, exit_code }) => {
                        tracing::info!(agent = %agent_id, ?exit_code, "managed agent exited");
                        app.agents.retain(|a| a.agent_id != *agent_id);
                        app.stopped_agents.remove(agent_id);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::HistoryResponse { entries }) => {
                        tracing::info!(count = entries.len(), "received history");
                        let page_size = HISTORY_PAGE_SIZE as usize;

                        match app.view_mode {
                            wisphive_tui::app::ViewMode::SessionTimeline => {
                                let offset = app.session_timeline_page * page_size;
                                let page_entries: Vec<_> = entries.into_iter().skip(offset).collect();
                                app.session_timeline_has_more = page_entries.len() > page_size;
                                app.session_timeline = page_entries.into_iter().take(page_size).collect();
                                app.session_timeline_index = 0;
                            }
                            _ => {
                                let offset = app.history_page * page_size;
                                let page_entries: Vec<_> = entries.into_iter().skip(offset).collect();
                                app.history_has_more = page_entries.len() > page_size;
                                app.history = page_entries.into_iter().take(page_size).collect();
                                app.history_index = 0;
                            }
                        }
                    }
                    Some(ServerMessage::SessionsResponse { sessions }) => {
                        tracing::info!(count = sessions.len(), "received sessions");
                        app.sessions = sessions;
                        app.sessions_index = 0;
                    }
                    Some(ServerMessage::ProjectsResponse { projects }) => {
                        tracing::info!(count = projects.len(), "received projects");
                        app.project_summaries = projects;
                        app.project_summaries_index = 0;
                    }
                    Some(_) => {}
                    None => {
                        tracing::warn!("daemon disconnected");
                        app.connected = false;
                        break;
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
