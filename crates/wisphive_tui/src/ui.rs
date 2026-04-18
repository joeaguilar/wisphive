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
        ViewMode::ProjectsExplorer => draw_projects_explorer(frame, app),
        ViewMode::SessionTimeline => draw_session_timeline_view(frame, app),
        ViewMode::TerminalList => draw_terminal_list_view(frame, app),
        ViewMode::TerminalView => draw_terminal_view(frame, app),
        ViewMode::TerminalReplay => draw_terminal_replay_view(frame, app),
        ViewMode::Dashboard => draw_dashboard(frame, app),
    }
}

fn draw_terminal_list_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let items: Vec<ListItem> = app
        .terminals
        .iter()
        .map(|t| {
            let label = t.label.as_deref().unwrap_or("(no label)");
            let status_color = match t.status {
                wisphive_protocol::TerminalStatus::Running => Color::Green,
                wisphive_protocol::TerminalStatus::Exited => Color::Gray,
                wisphive_protocol::TerminalStatus::Killed => Color::Red,
                wisphive_protocol::TerminalStatus::Orphaned => Color::Yellow,
            };
            let status = format!("{}", t.status);
            let line = Line::from(vec![
                Span::styled(format!("{:<10} ", status), Style::default().fg(status_color)),
                Span::styled(format!("{:<20} ", label), Style::default().fg(Color::White)),
                Span::raw(format!("{} ", t.command)),
                Span::styled(
                    format!("{}x{} ", t.cols, t.rows),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{}", t.started_at.format("%Y-%m-%d %H:%M:%S")),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let title = format!(" Terminals ({}) ", app.terminals.len());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("► ");

    let mut state = ratatui::widgets::ListState::default();
    if !app.terminals.is_empty() {
        state.select(Some(app.terminals_index));
    }
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let status = Paragraph::new(Line::from(vec![
        Span::styled("[n]", Style::default().fg(Color::Yellow)),
        Span::raw(" new  "),
        Span::styled("[P]", Style::default().fg(Color::Yellow)),
        Span::raw(" new in project  "),
        Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" attach  "),
        Span::styled("[r]", Style::default().fg(Color::Yellow)),
        Span::raw(" replay  "),
        Span::styled("[d]", Style::default().fg(Color::Yellow)),
        Span::raw(" close  "),
        Span::styled("[j/k]", Style::default().fg(Color::Yellow)),
        Span::raw(" move  "),
        Span::styled("[q/Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" back"),
    ]));
    frame.render_widget(status, chunks[1]);
}

fn draw_terminal_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    if let Some(active) = app.active_terminal.as_ref() {
        let screen = active.parser.screen();
        let title = format!(
            " Terminal: {} — {}{} ",
            active.label.as_deref().unwrap_or("(no label)"),
            active.command,
            if active.ended { " [ended]" } else { "" }
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title(title);
        let widget = tui_term::widget::PseudoTerminal::new(screen).block(block);
        frame.render_widget(widget, chunks[0]);
    } else {
        let msg = Paragraph::new("(no active terminal)")
            .block(Block::default().borders(Borders::ALL).title(" Terminal "));
        frame.render_widget(msg, chunks[0]);
    }

    let status = Paragraph::new(Line::from(vec![
        Span::styled("[F10]", Style::default().fg(Color::Yellow)),
        Span::raw(" detach  "),
        Span::styled("[Esc Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" detach  "),
        Span::styled("[Ctrl-C]", Style::default().fg(Color::Yellow)),
        Span::raw(" → PTY  "),
        Span::styled("all other keys", Style::default().fg(Color::Yellow)),
        Span::raw(" → session"),
    ]));
    frame.render_widget(status, chunks[1]);
}

fn draw_terminal_replay_view(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    if let Some(replay) = app.replay_terminal.as_ref() {
        let screen = replay.parser.screen();
        let title = format!(
            " Replay: {} — {} ",
            replay.label.as_deref().unwrap_or("(no label)"),
            replay.command
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .title(title);
        let widget = tui_term::widget::PseudoTerminal::new(screen).block(block);
        frame.render_widget(widget, chunks[0]);
    } else {
        let msg = Paragraph::new("(no replay active)")
            .block(Block::default().borders(Borders::ALL).title(" Replay "));
        frame.render_widget(msg, chunks[0]);
    }

    let status = Paragraph::new(Line::from(vec![
        Span::styled("[q/Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" back"),
    ]));
    frame.render_widget(status, chunks[1]);
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
        if modal.picker.is_some() {
            draw_picker_modal(frame, app);
        } else {
            draw_modal(frame, modal);
        }
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
        let lines = detail::render_detail_lines(req, app.markdown_preview);
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

        use wisphive_protocol::HookEventType;
        let bar_text = match req.hook_event_name {
            HookEventType::PermissionRequest => {
                if let Some(ref suggestions) = req.permission_suggestions {
                    let n = suggestions.len();
                    format!(" [1-{}]select [N]deny [M]deny+msg [?]defer [q/Esc]back [Q]uit{}", n, scroll_info)
                } else if req.tool_input.get("questions").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty()) {
                    // AskUserQuestion: PermissionRequest without suggestions
                    let n = req.tool_input
                        .get("questions")
                        .and_then(|v| v.as_array())
                        .and_then(|qs| qs.first())
                        .and_then(|q| q.get("options"))
                        .and_then(|v| v.as_array())
                        .map_or(0, |o| o.len());
                    format!(" [1-{}]select [O]ther [D]eny [M]deny+msg [q/Esc]back [Q]uit{}", n, scroll_info)
                } else {
                    // ExitPlanMode / generic PermissionRequest
                    format!(" [A/Enter]accept [D]eny [M]deny+msg [q/Esc]back [Q]uit{}", scroll_info)
                }
            }
            HookEventType::Stop | HookEventType::SubagentStop => {
                format!(" [A/Enter]accept [D]deny+msg [q/Esc]back [Q]uit{}", scroll_info)
            }
            HookEventType::UserPromptSubmit | HookEventType::ConfigChange => {
                format!(" [A]llow [B]lock [M]block+msg [q/Esc]back [Q]uit{}", scroll_info)
            }
            HookEventType::Elicitation => {
                format!(" [A]ccept [D]ecline [C]ancel [q/Esc]back [Q]uit{}", scroll_info)
            }
            HookEventType::TeammateIdle => {
                format!(" [C]ontinue+feedback [S]top [q/Esc]back [Q]uit{}", scroll_info)
            }
            HookEventType::TaskCompleted => {
                format!(" [A]ccept [R]eject+feedback [q/Esc]back [Q]uit{}", scroll_info)
            }
            _ => {
                format!(" [Y]approve [N]deny [M]deny+msg [!]always [E]edit [C]context [?]defer [q/Esc]back [Q]uit{}", scroll_info)
            }
        };
        let preview_indicator = if app.markdown_preview { " [P]raw" } else { " [P]review" };
        let bar = Paragraph::new(Line::from(Span::styled(
            format!("{bar_text}{preview_indicator}"),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        )));
        frame.render_widget(bar, chunks[1]);
    } else {
        let msg = Paragraph::new("Decision was resolved. Press Esc to return.")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, chunks[0]);
    }

    // Render modal overlay (deny-with-message, answer-question, etc.)
    if let Some(ref modal) = app.modal {
        if modal.picker.is_some() {
            draw_picker_modal(frame, app);
        } else {
            draw_modal(frame, modal);
        }
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
    use crate::app::{ALL_TOOLS, ConfigRow, EVENT_TOGGLES};

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
            ConfigRow::EventToggle(key) => {
                let display_name = EVENT_TOGGLES.iter()
                    .find(|(k, _)| *k == *key)
                    .map(|(_, name)| *name)
                    .unwrap_or(key);
                let enabled = app.config_event_toggles.get(*key).copied().unwrap_or(false);
                let checkbox = if enabled { "[x]" } else { "[ ]" };
                let name_style = if selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {arrow} {checkbox} "),
                        Style::default().fg(if enabled { Color::Green } else { Color::Red }),
                    ),
                    Span::styled(format!("{:<20}", display_name), name_style),
                    Span::styled(
                        if enabled { "auto-approve" } else { "review in queue" }.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
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

fn draw_projects_explorer(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let live_count = app.project_summaries.iter().filter(|p| p.has_live_agents).count();
    let title = format!(
        " Projects ({} total, {} active) ",
        app.project_summaries.len(),
        live_count
    );

    let items: Vec<ListItem> = app
        .project_summaries
        .iter()
        .enumerate()
        .map(|(i, project)| {
            let status = if project.has_live_agents {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };

            let project_name = project
                .project
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| project.project.to_string_lossy().to_string());

            let dur = project
                .last_seen
                .signed_duration_since(project.first_seen)
                .num_seconds();
            let dur_str = if dur < 60 {
                format!("{dur}s")
            } else if dur < 3600 {
                format!("{}m", dur / 60)
            } else {
                format!("{}h", dur / 3600)
            };

            let pending = if project.pending_count > 0 {
                Span::styled(
                    format!(" [{}!]", project.pending_count),
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
                    format!("{:<20} ", project_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{}agents ", project.agent_count),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(format!("{:>6} ", dur_str), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}calls ", project.total_calls),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{}ok ", project.approved),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("{}deny", project.denied),
                    Style::default().fg(Color::Red),
                ),
                pending,
            ]);

            let style = if i == app.project_summaries_index {
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
        " [j/k]navigate [Enter]activity [n]spawn agent [r]efresh [q/Esc]back [Q]uit ",
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
        .enumerate()
        .map(|(i, agent)| {
            let is_stopped = app.stopped_agents.contains(&agent.agent_id);
            let (dot, dot_style) = if is_stopped {
                ("■", Style::default().fg(Color::Red))
            } else {
                ("●", Style::default().fg(Color::Green))
            };
            let label = format!("{} {} ", agent.agent_id, agent.agent_type);
            let line = Line::from(vec![
                Span::styled(label, Style::default().fg(Color::White)),
                Span::styled(dot.to_string(), dot_style),
            ]);
            let style = if i == app.agents_index && focused {
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
        .enumerate()
        .map(|(i, p)| {
            let text = panels::format_project_status(p);
            let style = if i == app.projects_panel_index && focused {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(text)).style(style)
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
            " [y]approve [Enter/a/d]review [A]ll [D]eny-all [n]spawn [P]ick+spawn [h]istory [s]essions [p]rojects [c]onfig [/]filter [Tab]cycle [q]back [Q]uit | {} ",
            conn
        )
    };

    let bar = Paragraph::new(Line::from(Span::styled(
        status,
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));

    frame.render_widget(bar, area);
}

fn draw_picker_modal(frame: &mut Frame, app: &App) {
    let modal = app.modal.as_ref().unwrap();
    let picker = modal.picker.as_ref().unwrap();
    let area = frame.area();

    let project_count = app.project_summaries.len();
    // Height: 2 (title border + body) + max 12 project rows + 2 (bottom border + status)
    let list_height = project_count.clamp(1, 12) as u16;
    let modal_height = (list_height + 5).min(area.height.saturating_sub(4));
    let modal_width = 70.min(area.width.saturating_sub(4));

    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;
    let modal_area = Rect::new(x, y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} ", modal.title));

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if project_count == 0 {
        let msg = Paragraph::new(Line::from(Span::styled(
            "No projects found. Run an agent first to register a project.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // hint line
            Constraint::Min(1),   // project list
        ])
        .split(inner);

    let hint = Paragraph::new(Line::from(Span::styled(
        "j/k navigate, Enter select, Esc cancel",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(hint, chunks[0]);

    let items: Vec<ListItem> = app
        .project_summaries
        .iter()
        .enumerate()
        .map(|(i, project)| {
            let indicator = if project.has_live_agents {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };

            let project_name = project
                .project
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| project.project.to_string_lossy().to_string());

            let detail = format!("{}agents {}calls", project.agent_count, project.total_calls);

            let line = Line::from(vec![
                indicator,
                Span::styled(
                    format!("{:<20} ", project_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(detail, Style::default().fg(Color::DarkGray)),
            ]);

            let style = if i == picker.index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[1]);
}

fn draw_modal(frame: &mut Frame, modal: &Modal) {
    let area = frame.area();

    let has_input = modal.textarea.is_some() || modal.spawn.is_some();
    let modal_height = if modal.spawn.is_some() {
        // Spawn modal: body + project + prompt + options row (3 fields)
        15.min(area.height.saturating_sub(4))
    } else if has_input {
        // Single TextArea field: body + bordered input (3 lines)
        9.min(area.height.saturating_sub(4))
    } else {
        7.min(area.height.saturating_sub(4))
    };
    let modal_width = 80.min(area.width.saturating_sub(4));

    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(x, y, modal_width, modal_height);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} ", modal.title));

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    if let Some(ref textarea) = modal.textarea {
        // Body text + TextArea widget
        use ratatui::layout::{Constraint, Layout};
        let chunks = Layout::vertical([
            Constraint::Length(2), // body text + blank line
            Constraint::Min(3),   // TextArea
        ]).split(inner);

        let body = Paragraph::new(Line::from(Span::styled(
            modal.body.clone(),
            Style::default().fg(Color::White),
        )));
        frame.render_widget(body, chunks[0]);
        frame.render_widget(textarea, chunks[1]);
    } else if let Some(ref spawn) = modal.spawn {
        // Body text + project + prompt + options row (model | reasoning | max_turns)
        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::vertical([
            Constraint::Length(2), // body text
            Constraint::Length(3), // project
            Constraint::Length(3), // prompt
            Constraint::Min(3),   // model | reasoning | max_turns
        ]).split(inner);

        let body = Paragraph::new(Line::from(Span::styled(
            modal.body.clone(),
            Style::default().fg(Color::White),
        )));
        frame.render_widget(body, chunks[0]);
        frame.render_widget(&spawn.project, chunks[1]);
        frame.render_widget(&spawn.prompt, chunks[2]);

        let options_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Percentage(35),
                Constraint::Percentage(25),
            ])
            .split(chunks[3]);
        frame.render_widget(&spawn.model, options_row[0]);
        frame.render_widget(&spawn.reasoning, options_row[1]);
        frame.render_widget(&spawn.max_turns, options_row[2]);
    } else {
        // Simple Y/N confirmation — just body text
        let body = Paragraph::new(Line::from(Span::styled(
            modal.body.clone(),
            Style::default().fg(Color::White),
        )));
        frame.render_widget(body, inner);
    }
}
