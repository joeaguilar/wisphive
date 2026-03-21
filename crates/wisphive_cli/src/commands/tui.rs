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
                            tracing::info!(%id, "approved");
                            conn.send(&ClientMessage::Approve { id }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::Deny(id) => {
                            tracing::info!(%id, "denied");
                            conn.send(&ClientMessage::Deny { id }).await?;
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
                        InputAction::None => {}
                    }
                }
            }

            // Check for daemon messages
            msg = conn.recv() => {
                match msg? {
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
                        app.rebuild_projects();
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
