use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::{App, FocusPanel};
use crate::modal::Modal;
use crate::panels;

/// Render the entire TUI.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),    // Queue panel
            Constraint::Length(8), // Bottom panels (agents + projects)
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    draw_queue_panel(frame, app, chunks[0]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    draw_agents_panel(frame, app, bottom[0]);
    draw_projects_panel(frame, app, bottom[1]);
    draw_status_bar(frame, app, chunks[2]);

    // Draw modal on top if present
    if let Some(ref modal) = app.modal {
        draw_modal(frame, modal);
    }
}

fn draw_queue_panel(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == FocusPanel::Queue;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let filtered = app.filtered_queue();
    let title = if let Some(ref filter) = app.filter {
        format!(" Queue ({} pending, filter: {}) ", filtered.len(), filter)
    } else {
        format!(" Queue ({} pending) ", filtered.len())
    };

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, req)| {
            let content = panels::format_queue_item(req);
            let style = if i == app.queue_index && focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(content, style)))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );

    frame.render_widget(list, area);
}

fn draw_agents_panel(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == FocusPanel::Agents;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|agent| {
            let text = format!("{} {} ●", agent.agent_id, agent.agent_type);
            ListItem::new(Line::from(text))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" Agents ({}) ", app.agents.len())),
    );

    frame.render_widget(list, area);
}

fn draw_projects_panel(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == FocusPanel::Projects;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = app
        .projects
        .iter()
        .map(|p| {
            let text = panels::format_project_status(p);
            ListItem::new(Line::from(text))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" Projects ({}) ", app.projects.len())),
    );

    frame.render_widget(list, area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let status = if app.filter_input_mode {
        format!("/{}", app.filter_buffer)
    } else {
        let conn = if app.connected {
            "connected"
        } else {
            "disconnected"
        };
        format!(
            " [a]pprove [d]eny [A]ll-approve [D]all-deny [/]filter [Tab]cycle [q]uit | {} ",
            conn
        )
    };

    let bar = Paragraph::new(Line::from(Span::styled(
        status,
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));

    frame.render_widget(bar, area);
}

fn draw_modal(frame: &mut Frame, modal: &Modal) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 7.min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} ", modal.title));

    let text = Paragraph::new(modal.body.clone())
        .block(block)
        .style(Style::default().fg(Color::White));

    frame.render_widget(text, modal_area);
}
