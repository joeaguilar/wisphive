use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, FocusPanel, ViewMode};
use crate::detail;
use crate::modal::Modal;
use crate::panels;

/// Render the entire TUI.
pub fn draw(frame: &mut Frame, app: &App) {
    match app.view_mode {
        ViewMode::Detail => draw_detail_view(frame, app),
        ViewMode::History => draw_history_view(frame, app),
        ViewMode::HistoryDetail => draw_history_detail_view(frame, app),
        ViewMode::Dashboard => draw_dashboard(frame, app),
    }
}

fn draw_dashboard(frame: &mut Frame, app: &App) {
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

fn draw_detail_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Detail content (scrollable)
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    if let Some(req) = app.detail_request() {
        let lines = detail::render_detail_lines(req);
        let total_lines = lines.len();
        let visible_height = chunks[0].height.saturating_sub(2) as usize;

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(format!(" Review: {} ", req.tool_name)),
            )
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll as u16, 0));

        frame.render_widget(paragraph, chunks[0]);

        let scroll_info = if total_lines > visible_height {
            let max_scroll = total_lines.saturating_sub(visible_height);
            let pos = app.detail_scroll.min(max_scroll) + 1;
            format!(" [{}/{}]", pos, max_scroll + 1)
        } else {
            String::new()
        };

        let bar_text = format!(
            " [Y]approve [N]deny [Esc]back [j/k]scroll{}",
            scroll_info
        );
        let bar = Paragraph::new(Line::from(Span::styled(
            bar_text,
            Style::default().fg(Color::White).bg(Color::DarkGray),
        )));
        frame.render_widget(bar, chunks[1]);
    } else {
        let msg = Paragraph::new("Decision was resolved. Press Esc to return.")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, chunks[0]);
    }
}

fn draw_history_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // History list
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    let title = match (&app.history_agent_filter, &app.history_search_query) {
        (Some(agent), Some(query)) => format!(
            " History — agent: {} search: \"{}\" ({} entries) ",
            agent, query, app.history.len()
        ),
        (None, Some(query)) => format!(
            " History — search: \"{}\" ({} entries) ",
            query, app.history.len()
        ),
        (Some(agent), None) => format!(
            " History — agent: {} ({} entries) ",
            agent, app.history.len()
        ),
        (None, None) => format!(
            " History — all agents ({} entries) ",
            app.history.len()
        ),
    };

    let items: Vec<ListItem> = app
        .history
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let decision_str = match entry.decision {
                wisphive_protocol::Decision::Approve => "APPROVED",
                wisphive_protocol::Decision::Deny => "DENIED  ",
            };
            let decision_color = match entry.decision {
                wisphive_protocol::Decision::Approve => Color::Green,
                wisphive_protocol::Decision::Deny => Color::Red,
            };

            let project_name = entry
                .project
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.project.to_string_lossy().to_string());

            let time_str = entry.resolved_at.format("%m-%d %H:%M:%S").to_string();

            let result_indicator = if entry.tool_result.is_some() {
                Span::styled("+ ", Style::default().fg(Color::Cyan))
            } else {
                Span::styled("  ", Style::default())
            };

            let line = Line::from(vec![
                result_indicator,
                Span::styled(
                    format!("{decision_str} "),
                    Style::default().fg(decision_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<12} ", project_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:<8} ", entry.tool_name),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{} ", entry.agent_id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(time_str, Style::default().fg(Color::DarkGray)),
            ]);

            let style = if i == app.history_index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title),
    );

    frame.render_widget(list, chunks[0]);

    let bar_text = if app.history_search_mode {
        format!("/{}", app.history_search_buffer)
    } else {
        " [j/k]navigate [Enter]detail [/]search [C]clear-search [f]agent [F]clear [Esc]back [q]quit ".to_string()
    };
    let bar = Paragraph::new(Line::from(Span::styled(
        bar_text,
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(bar, chunks[1]);
}

fn draw_history_detail_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    if let Some(entry) = app.selected_history_entry() {
        let lines = detail::render_history_detail_lines(entry);
        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(format!(" History Detail: {} ", entry.tool_name)),
            )
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll as u16, 0));
        frame.render_widget(paragraph, chunks[0]);
    } else {
        let msg = Paragraph::new("No entry selected. Press Esc to return.")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, chunks[0]);
    }

    let bar = Paragraph::new(Line::from(Span::styled(
        " [j/k]scroll [Esc]back [q]quit ",
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(bar, chunks[1]);
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
            " [y]approve [Enter/a/d]review [A]pprove-all [D]eny-all [h]istory [/]filter [Tab]cycle [q]uit | {} ",
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
