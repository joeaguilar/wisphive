use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use wisphive_protocol::{DecisionRequest, HistoryEntry};

/// Render the full detail content for a DecisionRequest as styled Lines.
pub fn render_detail_lines(req: &DecisionRequest) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    push_header(&mut lines, req);
    lines.push(Line::from(""));

    use wisphive_protocol::HookEventType;
    match req.hook_event_name {
        HookEventType::PermissionRequest => {
            if let Some(ref suggestions) = req.permission_suggestions {
                push_permission_detail(&mut lines, req, suggestions);
            }
        }
        HookEventType::Elicitation => push_elicitation_detail(&mut lines, req),
        HookEventType::Stop | HookEventType::SubagentStop => push_event_data_detail(&mut lines, req, "Stop Reason"),
        HookEventType::UserPromptSubmit => push_event_data_detail(&mut lines, req, "Submitted Prompt"),
        HookEventType::ConfigChange => push_event_data_detail(&mut lines, req, "Config Change"),
        HookEventType::TeammateIdle => push_event_data_detail(&mut lines, req, "Teammate Status"),
        HookEventType::TaskCompleted => push_event_data_detail(&mut lines, req, "Task Completed"),
        _ => {
            // PreToolUse and unknown: tool-specific rendering
            match req.tool_name.to_lowercase().as_str() {
                "bash" => push_bash_detail(&mut lines, req),
                "edit" | "multiedit" => push_edit_detail(&mut lines, req),
                "write" => push_write_detail(&mut lines, req),
                "read" => push_read_detail(&mut lines, req),
                "grep" => push_grep_detail(&mut lines, req),
                "glob" => push_glob_detail(&mut lines, req),
                _ => push_generic_detail(&mut lines, req),
            }
        }
    }

    // PermissionRequest already renders its own action hints inline
    if req.hook_event_name != HookEventType::PermissionRequest {
        push_action_hints(&mut lines, req.hook_event_name);
    }

    lines
}

fn push_header(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    let label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(Color::White);

    let age = chrono::Utc::now()
        .signed_duration_since(req.timestamp)
        .num_seconds();
    let age_str = if age < 60 {
        format!("{age}s ago")
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else {
        format!("{}h ago", age / 3600)
    };

    lines.push(Line::from(vec![
        Span::styled("  Agent:    ", label_style),
        Span::styled(
            format!("{} ({})", req.agent_id, req.agent_type),
            value_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Project:  ", label_style),
        Span::styled(req.project.to_string_lossy().to_string(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Tool:     ", label_style),
        Span::styled(req.tool_name.clone(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Time:     ", label_style),
        Span::styled(
            format!("{} ({})", req.timestamp.format("%Y-%m-%d %H:%M:%S"), age_str),
            value_style,
        ),
    ]));
}

fn push_bash_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    if let Some(desc) = req.tool_input.get("description").and_then(|v| v.as_str()) {
        lines.push(Line::from(vec![
            Span::styled(
                "  Description: ",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(""));
    }

    push_section_label(lines, "Command");
    lines.push(Line::from(""));

    if let Some(cmd) = req.tool_input.get("command").and_then(|v| v.as_str()) {
        for line in cmd.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::Yellow),
            )));
        }
    } else {
        push_json_fallback(lines, &req.tool_input);
    }
}

fn push_edit_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    if let Some(path) = req.tool_input.get("file_path").and_then(|v| v.as_str()) {
        push_file_label(lines, path);
    }

    let old_text = req
        .tool_input
        .get("old_string")
        .or_else(|| req.tool_input.get("old_text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_text = req
        .tool_input
        .get("new_string")
        .or_else(|| req.tool_input.get("new_text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if old_text.is_empty() && new_text.is_empty() {
        push_json_fallback(lines, &req.tool_input);
        return;
    }

    push_section_label(lines, "Changes");
    lines.push(Line::from(""));
    push_diff_lines(lines, old_text, new_text);
}

fn push_write_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    if let Some(path) = req.tool_input.get("file_path").and_then(|v| v.as_str()) {
        push_file_label(lines, path);
    }

    push_section_label(lines, "Content (new file)");
    lines.push(Line::from(""));

    if let Some(content) = req.tool_input.get("content").and_then(|v| v.as_str()) {
        let green = Style::default().fg(Color::Green);
        let gutter = Style::default().fg(Color::DarkGray);
        for (i, line) in content.lines().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:>4} ", i + 1), gutter),
                Span::styled(line.to_string(), green),
            ]));
        }
    } else {
        push_json_fallback(lines, &req.tool_input);
    }
}

fn push_read_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    if let Some(path) = req.tool_input.get("file_path").and_then(|v| v.as_str()) {
        push_file_label(lines, path);
    }
    push_field_if_present(lines, &req.tool_input, "limit", "Limit");
    push_field_if_present(lines, &req.tool_input, "offset", "Offset");
}

fn push_grep_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    push_field_if_present(lines, &req.tool_input, "pattern", "Pattern");
    push_field_if_present(lines, &req.tool_input, "path", "Path");
    push_field_if_present(lines, &req.tool_input, "type", "Type");
    push_field_if_present(lines, &req.tool_input, "glob", "Glob");
    push_field_if_present(lines, &req.tool_input, "output_mode", "Output");
}

fn push_glob_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    push_field_if_present(lines, &req.tool_input, "pattern", "Pattern");
    push_field_if_present(lines, &req.tool_input, "path", "Path");
}

fn push_generic_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    push_section_label(lines, "Tool Input");
    lines.push(Line::from(""));
    push_json_fallback(lines, &req.tool_input);
}

fn push_elicitation_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    if let Some(ref data) = req.event_data {
        if let Some(server) = data.get("mcp_server_name").and_then(|v| v.as_str()) {
            push_field_line(lines, "MCP Server", server);
        }
        if let Some(mode) = data.get("mode").and_then(|v| v.as_str()) {
            push_field_line(lines, "Mode", mode);
        }
        if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
            push_field_line(lines, "Message", msg);
        }
        if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
            push_field_line(lines, "URL", url);
        }
        if let Some(schema) = data.get("requested_schema") {
            push_section_label(lines, "Requested Schema");
            lines.push(Line::from(""));
            push_json_fallback(lines, schema);
        }
    }
    if !req.tool_input.is_null() {
        lines.push(Line::from(""));
        push_section_label(lines, "Tool Input");
        lines.push(Line::from(""));
        push_json_fallback(lines, &req.tool_input);
    }
}

fn push_event_data_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest, label: &str) {
    push_section_label(lines, label);
    lines.push(Line::from(""));
    if let Some(ref data) = req.event_data {
        push_json_fallback(lines, data);
    }
    if !req.tool_input.is_null() {
        lines.push(Line::from(""));
        push_section_label(lines, "Tool Input");
        lines.push(Line::from(""));
        push_json_fallback(lines, &req.tool_input);
    }
}

fn push_field_line(lines: &mut Vec<Line<'static>>, label: &str, value: &str) {
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {label}: "),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ]));
}

/// Render the full detail content for a HistoryEntry (including tool_result).
pub fn render_history_detail_lines(entry: &HistoryEntry) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(Color::White);

    let decision_str = match entry.decision {
        wisphive_protocol::Decision::Approve => "APPROVED",
        wisphive_protocol::Decision::Deny => "DENIED",
        wisphive_protocol::Decision::Ask => "DEFERRED",
    };
    let decision_color = match entry.decision {
        wisphive_protocol::Decision::Approve => Color::Green,
        wisphive_protocol::Decision::Deny => Color::Red,
        wisphive_protocol::Decision::Ask => Color::Yellow,
    };

    lines.push(Line::from(vec![
        Span::styled("  Decision: ", label_style),
        Span::styled(
            decision_str.to_string(),
            Style::default()
                .fg(decision_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Agent:    ", label_style),
        Span::styled(
            format!("{} ({})", entry.agent_id, entry.agent_type),
            value_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Project:  ", label_style),
        Span::styled(entry.project.to_string_lossy().to_string(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Tool:     ", label_style),
        Span::styled(entry.tool_name.clone(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Requested:", label_style),
        Span::styled(
            format!(" {}", entry.requested_at.format("%Y-%m-%d %H:%M:%S")),
            value_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Resolved: ", label_style),
        Span::styled(
            format!(" {}", entry.resolved_at.format("%Y-%m-%d %H:%M:%S")),
            value_style,
        ),
    ]));
    lines.push(Line::from(""));

    // Tool Input
    push_section_label(&mut lines, "Tool Input");
    lines.push(Line::from(""));
    push_json_fallback(&mut lines, &entry.tool_input);
    lines.push(Line::from(""));

    // Tool Result
    push_section_label(&mut lines, "Tool Result");
    lines.push(Line::from(""));
    if let Some(ref result) = entry.tool_result {
        push_json_fallback(&mut lines, result);
    } else {
        lines.push(Line::from(Span::styled(
            "  (not captured)".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

fn push_permission_detail(
    lines: &mut Vec<Line<'static>>,
    req: &DecisionRequest,
    suggestions: &[wisphive_protocol::PermissionSuggestion],
) {
    push_section_label(lines, "Permission Request");
    lines.push(Line::from(""));

    // Show tool input context
    if let Some(cmd) = req.tool_input.get("command").and_then(|v| v.as_str()) {
        lines.push(Line::from(vec![
            Span::styled(
                "  Command: ",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(cmd.to_string(), Style::default().fg(Color::Yellow)),
        ]));
        lines.push(Line::from(""));
    }

    push_section_label(lines, "Available Options");
    lines.push(Line::from(""));

    for (i, suggestion) in suggestions.iter().enumerate() {
        let rules_str = if suggestion.rules.is_empty() {
            suggestion
                .mode
                .as_deref()
                .unwrap_or(&suggestion.suggestion_type)
                .to_string()
        } else {
            suggestion
                .rules
                .iter()
                .map(|r| format!("{}({})", r.tool_name, r.rule_content))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let behavior_color = if suggestion.behavior == "allow" {
            Color::Green
        } else {
            Color::Red
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  [{}] ", i + 1),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", suggestion.behavior.to_uppercase()),
                Style::default()
                    .fg(behavior_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(rules_str, Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "      {} → {}",
                suggestion.suggestion_type, suggestion.destination
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press a number to select, N to deny, M to deny with message",
        Style::default().fg(Color::DarkGray),
    )));
}

fn push_action_hints(lines: &mut Vec<Line<'static>>, event_type: wisphive_protocol::HookEventType) {
    use wisphive_protocol::HookEventType;

    lines.push(Line::from(""));
    push_section_label(lines, "Actions");
    lines.push(Line::from(""));

    let hint_style = Style::default().fg(Color::DarkGray);
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let actions: Vec<(&str, &str)> = match event_type {
        HookEventType::Stop | HookEventType::SubagentStop => vec![
            ("C", "continue working"),
            ("M", "continue with feedback"),
            ("S", "let agent stop"),
        ],
        HookEventType::UserPromptSubmit | HookEventType::ConfigChange => vec![
            ("A", "allow"),
            ("B", "block"),
            ("M", "block with message"),
        ],
        HookEventType::Elicitation => vec![
            ("A", "accept"),
            ("D", "decline"),
            ("C", "cancel"),
        ],
        HookEventType::TeammateIdle => vec![
            ("C", "continue with feedback"),
            ("S", "stop teammate"),
        ],
        HookEventType::TaskCompleted => vec![
            ("A", "accept"),
            ("R", "reject with feedback"),
        ],
        _ => vec![
            ("Y", "approve"),
            ("N", "deny"),
            ("M", "deny with message"),
            ("!", "always allow"),
            ("E", "edit input"),
            ("C", "add context"),
            ("?", "defer to native prompt"),
        ],
    };

    for (key, desc) in &actions {
        lines.push(Line::from(vec![
            Span::styled("  ", hint_style),
            Span::styled(format!("[{key}]"), key_style),
            Span::styled(format!(" {desc}"), hint_style),
        ]));
    }
}

// --- Helpers ---

fn push_section_label(lines: &mut Vec<Line<'static>>, label: &str) {
    lines.push(Line::from(Span::styled(
        format!("  ── {label} ──"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
}

fn push_file_label(lines: &mut Vec<Line<'static>>, path: &str) {
    lines.push(Line::from(vec![
        Span::styled(
            "  File: ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(path.to_string(), Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(""));
}

fn push_field_if_present(
    lines: &mut Vec<Line<'static>>,
    input: &serde_json::Value,
    key: &str,
    label: &str,
) {
    if let Some(val) = input.get(key) {
        let display = match val.as_str() {
            Some(s) => s.to_string(),
            None => val.to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {label}: "),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(display, Style::default().fg(Color::White)),
        ]));
    }
}

fn push_json_fallback(lines: &mut Vec<Line<'static>>, value: &serde_json::Value) {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    for line in pretty.lines() {
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(Color::White),
        )));
    }
}

fn push_diff_lines(lines: &mut Vec<Line<'static>>, old_text: &str, new_text: &str) {
    let diff = TextDiff::from_lines(old_text, new_text);

    for change in diff.iter_all_changes() {
        let (sign, style) = match change.tag() {
            ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
            ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
            ChangeTag::Equal => (" ", Style::default().fg(Color::DarkGray)),
        };
        let text = format!("  {sign} {}", change.value().trim_end_matches('\n'));
        lines.push(Line::from(Span::styled(text, style)));
    }
}
