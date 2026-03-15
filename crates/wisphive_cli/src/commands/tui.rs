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

    // Connect to daemon
    let mut conn = DaemonConnection::connect(&config.socket_path).await?;
    let mut app = App::new();
    app.connected = true;

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app, &mut conn).await;

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

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
                            conn.send(&ClientMessage::Approve { id }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::Deny(id) => {
                            conn.send(&ClientMessage::Deny { id }).await?;
                            app.remove_decision(id);
                        }
                        InputAction::ApproveAll => {
                            conn.send(&ClientMessage::ApproveAll { filter: None }).await?;
                            app.queue.clear();
                            app.queue_index = 0;
                            app.rebuild_projects();
                        }
                        InputAction::DenyAll => {
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
                    Some(ServerMessage::QueueSnapshot { items }) => {
                        app.queue = items;
                        app.queue_index = 0;
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::NewDecision(req)) => {
                        app.queue.push(req);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::DecisionResolved { id, .. }) => {
                        app.remove_decision(id);
                    }
                    Some(ServerMessage::AgentConnected(info)) => {
                        app.agents.push(info);
                        app.rebuild_projects();
                    }
                    Some(ServerMessage::AgentDisconnected { agent_id }) => {
                        app.agents.retain(|a| a.agent_id != agent_id);
                        app.rebuild_projects();
                    }
                    Some(_) => {} // Other messages ignored
                    None => {
                        app.connected = false;
                        break; // Daemon disconnected
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
