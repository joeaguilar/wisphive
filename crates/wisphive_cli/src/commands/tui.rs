use std::fmt::Display;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use crossterm::event;
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use wisphive_daemon::DaemonConfig;
use wisphive_protocol::{ClientMessage, ServerMessage};
use wisphive_tui::app::{App, ConfigMutation};
use wisphive_tui::connection::DaemonConnection;
use wisphive_tui::input::{self, InputAction};
use wisphive_tui::ui;

const HISTORY_PAGE_SIZE: u32 = 50;

fn persist_tui_config(
    path: &Path,
    mutation: ConfigMutation,
) -> Result<(), wisphive_daemon::ConfigUpdateError> {
    wisphive_daemon::update_config_json(path, |obj| mutation.apply_to(obj))
}

fn report_tui_config_save_error(app: &mut App, error: impl Display) {
    app.set_status_error(format!("Config save failed: {error}"));
}

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
    let config_path = config.config_json_path();
    let result = run_loop(&mut terminal, &mut app, &mut conn, &config_path).await;

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
    config_path: &Path,
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
                            if let Some(req) = app.queue.iter().find(|r| r.id == id)
                                && matches!(req.hook_event_name,
                                    wisphive_protocol::HookEventType::Stop
                                    | wisphive_protocol::HookEventType::SubagentStop
                                ) {
                                    app.stopped_agents.insert(req.agent_id.clone());
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
                            conn.send(&ClientMessage::ApproveAll {
                                filter: None,
                                // Only reachable through the bulk-approve
                                // confirm modal (Y pressed), so the explicit
                                // confirmation is genuine (itr#88).
                                confirm: true,
                            })
                            .await?;
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
                                request_id: None,
                            }).await?;
                        }
                        InputAction::QueryHistoryPage { agent_id, page } => {
                            tracing::info!(?agent_id, page, "querying history page");
                            let offset = page as u32 * HISTORY_PAGE_SIZE;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id,
                                limit: Some(offset + HISTORY_PAGE_SIZE + 1),
                                request_id: None,
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
                        InputAction::SaveConfig(mutation) => {
                            let path = config_path.to_owned();
                            let display_path = path.clone();
                            match tokio::task::spawn_blocking(move || {
                                persist_tui_config(&path, mutation)
                            })
                            .await
                            {
                                Ok(Ok(())) => {
                                    app.clear_status_error();
                                    tracing::info!(
                                        path = %display_path.display(),
                                        "saved TUI config"
                                    );
                                }
                                Ok(Err(error)) => {
                                    tracing::error!(
                                        path = %display_path.display(),
                                        %error,
                                        "failed to save TUI config; fix the on-disk config and retry"
                                    );
                                    report_tui_config_save_error(app, error);
                                }
                                Err(error) => {
                                    tracing::error!(
                                        path = %display_path.display(),
                                        %error,
                                        "TUI config save worker failed; retry the change"
                                    );
                                    report_tui_config_save_error(app, error);
                                }
                            }
                        }
                        InputAction::QuerySessionTimeline { agent_id } => {
                            tracing::info!(%agent_id, "querying session timeline");
                            app.session_timeline_page = 0;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id: Some(agent_id),
                                limit: Some(HISTORY_PAGE_SIZE + 1),
                                request_id: None,
                            }).await?;
                        }
                        InputAction::QuerySessionTimelinePage { agent_id, page } => {
                            tracing::info!(%agent_id, page, "querying session timeline page");
                            let offset = page as u32 * HISTORY_PAGE_SIZE;
                            conn.send(&ClientMessage::QueryHistory {
                                agent_id: Some(agent_id),
                                limit: Some(offset + HISTORY_PAGE_SIZE + 1),
                                request_id: None,
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
                        InputAction::TermList => {
                            tracing::info!("querying terminal sessions");
                            conn.send(&ClientMessage::TermList).await?;
                        }
                        InputAction::TermNew { label, cwd } => {
                            tracing::info!(?label, ?cwd, "creating terminal session");
                            // Query a sensible initial pty size from the
                            // current terminal. Fallback is 80x24.
                            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                            conn.send(&ClientMessage::TermCreate {
                                label,
                                command: None,
                                args: None,
                                cwd,
                                cols,
                                rows,
                                env: None,
                            }).await?;
                        }
                        InputAction::TermAttach { id } => {
                            tracing::info!(%id, "attaching terminal session");
                            conn.send(&ClientMessage::TermAttach { id }).await?;
                        }
                        InputAction::TermDetach { id } => {
                            tracing::info!(%id, "detaching terminal session");
                            conn.send(&ClientMessage::TermDetach { id }).await?;
                        }
                        InputAction::TermClose { id } => {
                            tracing::info!(%id, "closing terminal session");
                            conn.send(&ClientMessage::TermClose { id }).await?;
                        }
                        InputAction::TermInput { id, bytes } => {
                            use base64::Engine as _;
                            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                            conn.send(&ClientMessage::TermInput { id, data }).await?;
                        }
                        InputAction::TermReplay { id } => {
                            tracing::info!(%id, "replaying terminal session");
                            conn.send(&ClientMessage::TermReplay { id, from_seq: None, speed: None }).await?;
                        }
                        InputAction::None => {}
                    }
                }
            }

            // Check for daemon messages
            msg = conn.recv() => {
                // `recv` filters per-frame decode failures; an error here is
                // a real socket I/O failure and should still exit the TUI.
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
                    Some(ServerMessage::HistoryResponse { entries, .. }) => {
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
                    Some(ServerMessage::ReimportComplete { count }) => {
                        tracing::info!(count, "reimport complete");
                    }
                    Some(ServerMessage::TermCreated(meta)) => {
                        tracing::info!(id = %meta.id, "terminal session created");
                        // Auto-attach: enter the view and request the stream.
                        let id = meta.id;
                        app.terminals.insert(0, meta.clone());
                        app.enter_terminal_view(&meta);
                        conn.send(&ClientMessage::TermAttach { id }).await?;
                    }
                    Some(ServerMessage::TermListResponse { sessions }) => {
                        tracing::info!(count = sessions.len(), "terminal list response");
                        app.terminals = sessions;
                        if app.terminals_index >= app.terminals.len() {
                            app.terminals_index = 0;
                        }
                    }
                    Some(ServerMessage::TermChunk { id, seq, direction, data, .. }) => {
                        use base64::Engine as _;
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&data) {
                            // Only feed the active terminal (not replay).
                            if let Some(active) = app.active_terminal.as_mut()
                                && active.id == id
                                && matches!(direction, wisphive_protocol::TerminalDirection::Output)
                            {
                                active.feed_chunk(seq, &bytes);
                            }
                        }
                    }
                    Some(ServerMessage::TermCatchup { id, cols, rows, next_seq, screen }) => {
                        use base64::Engine as _;
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&screen)
                            && let Some(active) = app.active_terminal.as_mut()
                            && active.id == id {
                                active.resize(cols, rows);
                                active.last_seq = next_seq;
                                active.feed_catchup(&bytes);
                            }
                    }
                    Some(ServerMessage::TermEnded { id, status, .. }) => {
                        tracing::info!(%id, ?status, "terminal ended");
                        if let Some(active) = app.active_terminal.as_mut()
                            && active.id == id {
                                active.ended = true;
                            }
                        if let Some(meta) = app.terminals.iter_mut().find(|m| m.id == id) {
                            meta.status = status;
                        }
                    }
                    Some(ServerMessage::TermReplayChunk { id, direction, data, .. }) => {
                        use base64::Engine as _;
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&data)
                            && let Some(replay) = app.replay_terminal.as_mut()
                            && replay.id == id
                            && matches!(direction, wisphive_protocol::TerminalDirection::Output)
                        {
                            replay.parser.process(&bytes);
                        }
                    }
                    Some(ServerMessage::TermReplayDone { id, .. }) => {
                        tracing::info!(%id, "terminal replay done");
                    }
                    Some(ServerMessage::TermError { id, message }) => {
                        tracing::warn!(?id, %message, "terminal error");
                    }
                    Some(ServerMessage::DiskAlert { kind, active, message, .. }) => {
                        app.apply_disk_alert(kind, active, message);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::{Arc, Barrier};

    #[test]
    fn non_object_config_save_sets_status_error_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "[]").unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            persist_tui_config(
                &path,
                ConfigMutation::AutoApproveLevel(wisphive_protocol::AutoApproveLevel::Execute),
            )
        }));
        let error = result
            .expect("a non-object config must not panic")
            .expect_err("a non-object config must be refused");
        assert!(matches!(
            error,
            wisphive_daemon::ConfigUpdateError::NotAnObject
        ));

        let mut app = App::new();
        report_tui_config_save_error(&mut app, error);
        assert_eq!(
            app.status_error.as_deref(),
            Some("Config save failed: config file top level is not a JSON object")
        );

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("test backend is valid");
        terminal
            .draw(|frame| wisphive_tui::ui::draw(frame, &app))
            .expect("status error must render without panicking");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("ERROR: Config save failed"));
    }

    #[test]
    fn concurrent_tui_rule_transactions_preserve_same_tool_allow_and_deny() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "tool_rules": {"Bash": {"future_rule_field": {"keep": true}}},
                "future_top_level": true,
            })
            .to_string(),
        )
        .unwrap();

        let start = Arc::new(Barrier::new(3));
        let deny_start = start.clone();
        let deny_path = path.clone();
        let deny = std::thread::spawn(move || {
            deny_start.wait();
            persist_tui_config(
                &deny_path,
                ConfigMutation::ToolRulePattern {
                    tool: "Bash".into(),
                    pattern: "sudo".into(),
                    deny: true,
                    include: true,
                },
            )
        });
        let allow_start = start.clone();
        let allow_path = path.clone();
        let allow = std::thread::spawn(move || {
            allow_start.wait();
            persist_tui_config(
                &allow_path,
                ConfigMutation::ToolRulePattern {
                    tool: "Bash".into(),
                    pattern: "cargo test".into(),
                    deny: false,
                    include: true,
                },
            )
        });
        start.wait();
        deny.join().unwrap().unwrap();
        allow.join().unwrap().unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            after["tool_rules"]["Bash"]["deny_patterns"],
            serde_json::json!(["sudo"])
        );
        assert_eq!(
            after["tool_rules"]["Bash"]["allow_patterns"],
            serde_json::json!(["cargo test"])
        );
        assert_eq!(
            after["tool_rules"]["Bash"]["future_rule_field"]["keep"],
            true
        );
        assert_eq!(after["future_top_level"], true);
    }
}
