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
        ViewMode::Config => draw_config_view(frame, app),
        ViewMode::Sessions => draw_sessions_view(frame, app),
        ViewMode::SessionTimeline => draw_session_timeline_view(frame, app),
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

        let is_permission = req.permission_suggestions.is_some();
        let bar_text = if is_permission {
            let n = req.permission_suggestions.as_ref().map_or(0, |s| s.len());
            format!(
                " [1-{}]select [N]deny [M]deny+msg [?]defer [q/Esc]back [Q]uit{}",
                n, scroll_info
            )
        } else {
            format!(
                " [Y]approve [N]deny [M]deny+msg [!]always [E]edit [C]context [?]defer [q/Esc]back [Q]uit{}",
                scroll_info
            )
        };
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
                wisphive_protocol::Decision::Ask => "DEFERRED",
            };
            let decision_color = match entry.decision {
                wisphive_protocol::Decision::Approve => Color::Green,
                wisphive_protocol::Decision::Deny => Color::Red,
                wisphive_protocol::Decision::Ask => Color::Yellow,
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

    let page_info = if app.history_page > 0 || app.history_has_more {
        format!(" pg {} ", app.history_page + 1)
    } else {
        String::new()
    };
    let bar_text = if app.history_search_mode {
        format!("/{}", app.history_search_buffer)
    } else {
        format!(
            " [j/k]navigate [Enter]detail [/]search [H/[]prev [L/]]next [C]clear [f]agent [F]clear [q]back [Q]uit{}",
            page_info
        )
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
        " [j/k]scroll [q/Esc]back [Q]uit ",
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(bar, chunks[1]);
}

fn draw_config_view(frame: &mut Frame, app: &App) {
    use crate::app::{ALL_TOOLS, ConfigRow};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let rows = app.config_rows();
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (row_i, row) in rows.iter().enumerate() {
        let selected = app.config_index == row_i;
        let arrow = if selected { ">" } else { " " };

        match row {
            ConfigRow::Level => {
                let level_style = if selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{arrow} Level: "),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("◀ {} ▶", app.config_level), level_style),
                    Span::styled("  (use ←/→ to change)", Style::default().fg(Color::DarkGray)),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  Tools (Space/Enter toggle, + add rule, - remove rule):",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            ConfigRow::Tool(tool_idx) => {
                let tool = ALL_TOOLS[*tool_idx];
                let in_level = app.config_level.includes(tool);
                let in_add = app.config_add.iter().any(|t| t == tool);
                let in_remove = app.config_remove.iter().any(|t| t == tool);

                let (approved, status, color) = if in_remove {
                    (false, "QUEUED (override)", Color::Red)
                } else if in_add {
                    (true, "AUTO (override)", Color::Green)
                } else if in_level {
                    (true, "AUTO (level)", Color::DarkGray)
                } else {
                    (false, "QUEUED", Color::DarkGray)
                };

                let has_rules = app
                    .config_tool_rules
                    .get(tool)
                    .map(|r| !r.deny_patterns.is_empty() || !r.allow_patterns.is_empty())
                    .unwrap_or(false);

                let checkbox = if approved { "[x]" } else { "[ ]" };
                let name_style = if selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let mut spans = vec![
                    Span::styled(
                        format!("  {arrow} {checkbox} "),
                        Style::default().fg(if approved { Color::Green } else { Color::Red }),
                    ),
                    Span::styled(format!("{:<16}", tool), name_style),
                    Span::styled(status.to_string(), Style::default().fg(color)),
                ];
                if has_rules {
                    spans.push(Span::styled(" ⚙", Style::default().fg(Color::Cyan)));
                }
                lines.push(Line::from(spans));
            }
            ConfigRow::Rule { tool_idx, rule_idx, is_deny } => {
                let tool = ALL_TOOLS[*tool_idx];
                let pattern = if let Some(rule) = app.config_tool_rules.get(tool) {
                    if *is_deny {
                        rule.deny_patterns.get(*rule_idx).cloned().unwrap_or_default()
                    } else {
                        rule.allow_patterns.get(*rule_idx).cloned().unwrap_or_default()
                    }
                } else {
                    String::new()
                };

                let (label, label_color) = if *is_deny {
                    ("deny", Color::Red)
                } else {
                    ("allow", Color::Green)
                };

                let pat_style = if selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("        {arrow} "), Style::default()),
                    Span::styled(format!("{label}: "), Style::default().fg(label_color)),
                    Span::styled(format!("\"{pattern}\""), pat_style),
                ]));
            }
        }
    }

    // If in rule input mode, show the input line
    if app.config_rule_input_mode {
        let label = if app.config_rule_is_deny { "deny" } else { "allow" };
        let tool = app.config_rule_target_tool.as_deref().unwrap_or("?");
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  Add {label} pattern for {tool}: "),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{}_", app.config_rule_buffer),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Auto-Approve Config "),
        )
        .scroll((0, 0));

    frame.render_widget(paragraph, chunks[0]);

    let bar_text = if app.config_rule_input_mode {
        " Type pattern, Enter to add, Esc to cancel ".to_string()
    } else {
        " [j/k]nav [←/→]level [Space]toggle [+]add rule [-]del rule [q]back ".to_string()
    };
    let bar = Paragraph::new(Line::from(Span::styled(
        bar_text,
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(bar, chunks[1]);
}

fn draw_sessions_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let live_count = app.sessions.iter().filter(|s| s.is_live).count();
    let title = format!(
        " Sessions ({} total, {} live) ",
        app.sessions.len(),
        live_count
    );

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let status = if session.is_live {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };

            let project_name = session
                .project
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| session.project.to_string_lossy().to_string());

            let dur = session
                .last_seen
                .signed_duration_since(session.first_seen)
                .num_seconds();
            let dur_str = if dur < 60 {
                format!("{dur}s")
            } else if dur < 3600 {
                format!("{}m", dur / 60)
            } else {
                format!("{}h{}m", dur / 3600, (dur % 3600) / 60)
            };

            let pending = if session.pending_count > 0 {
                Span::styled(
                    format!(" [{}!]", session.pending_count),
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("", Style::default())
            };

            let line = Line::from(vec![
                status,
                Span::styled(
                    format!("{:<18} ", session.agent_id),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:<12} ", project_name),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(format!("{:>6} ", dur_str), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}calls ", session.total_calls),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{}ok ", session.approved),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("{}deny", session.denied),
                    Style::default().fg(Color::Red),
                ),
                pending,
            ]);

            let style = if i == app.sessions_index {
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

    let bar = Paragraph::new(Line::from(Span::styled(
        " [j/k]navigate [Enter]timeline [r]efresh [q/Esc]back [Q]uit ",
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(bar, chunks[1]);
}

fn draw_session_timeline_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let agent_id = app
        .session_timeline_agent_id
        .as_deref()
        .unwrap_or("unknown");
    let title = format!(
        " Session Timeline: {} ({} entries) ",
        agent_id,
        app.session_timeline.len()
    );

    let items: Vec<ListItem> = app
        .session_timeline
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let decision_str = match entry.decision {
                wisphive_protocol::Decision::Approve => "APPROVED",
                wisphive_protocol::Decision::Deny => "DENIED  ",
                wisphive_protocol::Decision::Ask => "DEFERRED",
            };
            let decision_color = match entry.decision {
                wisphive_protocol::Decision::Approve => Color::Green,
                wisphive_protocol::Decision::Deny => Color::Red,
                wisphive_protocol::Decision::Ask => Color::Yellow,
            };

            let result_indicator = if entry.tool_result.is_some() {
                Span::styled("+ ", Style::default().fg(Color::Cyan))
            } else {
                Span::styled("  ", Style::default())
            };

            let time_str = entry.resolved_at.format("%H:%M:%S").to_string();

            let input_summary = if let Some(cmd) =
                entry.tool_input.get("command").and_then(|v| v.as_str())
            {
                let s = cmd.to_string();
                if s.len() > 40 {
                    format!("{}...", &s[..37])
                } else {
                    s
                }
            } else if let Some(path) =
                entry.tool_input.get("file_path").and_then(|v| v.as_str())
            {
                path.to_string()
            } else {
                String::new()
            };

            let line = Line::from(vec![
                result_indicator,
                Span::styled(
                    format!("{decision_str} "),
                    Style::default()
                        .fg(decision_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<12} ", entry.tool_name),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(input_summary, Style::default().fg(Color::White)),
                Span::styled(
                    format!("  {time_str}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let style = if i == app.session_timeline_index {
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

    let page_info = if app.session_timeline_page > 0 || app.session_timeline_has_more {
        format!(" pg {} ", app.session_timeline_page + 1)
    } else {
        String::new()
    };
    let bar = Paragraph::new(Line::from(Span::styled(
        format!(
            " [j/k]navigate [Enter]detail [H/[]prev [L/]]next [q/Esc]back [Q]uit{}",
            page_info
        ),
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
            " [y]approve [Enter/a/d]review [A]ll [D]eny-all [h]istory [s]essions [c]onfig [/]filter [Tab]cycle [q]back [Q]uit | {} ",
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

    // Text/edit input modals are taller
    let has_input = modal.text_input.is_some() || modal.edit_input.is_some() || modal.spawn.is_some();
    let modal_height = if has_input {
        10.min(area.height.saturating_sub(4))
    } else {
        7.min(area.height.saturating_sub(4))
    };
    let modal_width = 60.min(area.width.saturating_sub(4));

    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} ", modal.title));

    // Build content lines
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        modal.body.clone(),
        Style::default().fg(Color::White),
    ))];

    if let Some(ref text_input) = modal.text_input {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{}_", text_input.buffer),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    } else if let Some(ref edit_input) = modal.edit_input {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{}_", edit_input.buffer),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    } else if let Some(ref spawn) = modal.spawn {
        lines.push(Line::from(""));
        let proj_style = if spawn.active_field == crate::modal::SpawnField::Project {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        let prompt_style = if spawn.active_field == crate::modal::SpawnField::Prompt {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}_", spawn.project_buf), proj_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Prompt:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}_", spawn.prompt_buf), prompt_style),
        ]));
    }

    let text = Paragraph::new(lines).block(block);
    frame.render_widget(text, modal_area);
}
