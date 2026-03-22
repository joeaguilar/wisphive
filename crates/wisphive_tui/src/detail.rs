use pulldown_cmark::{Event as MdEvent, Tag, TagEnd, Parser};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use wisphive_protocol::{DecisionRequest, HistoryEntry};

/// Render the full detail content for a DecisionRequest as styled Lines.
pub fn render_detail_lines(req: &DecisionRequest, markdown_preview: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    push_header(&mut lines, req);
    lines.push(Line::from(""));

    use wisphive_protocol::HookEventType;
    match req.hook_event_name {
        HookEventType::PermissionRequest => {
            if let Some(ref suggestions) = req.permission_suggestions {
                push_permission_detail(&mut lines, req, suggestions);
            } else if has_ask_questions(&req.tool_input) {
                push_ask_question_detail(&mut lines, req);
            } else if has_plan_content(req) {
                push_plan_detail(&mut lines, req, markdown_preview);
            } else {
                push_generic_detail(&mut lines, req);
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
                "askuserquestion" if has_ask_questions(&req.tool_input) => {
                    push_ask_question_detail(&mut lines, req)
                }
                _ => push_generic_detail(&mut lines, req),
            }
        }
    }

    // PermissionRequest with suggestions renders its own action hints inline;
    // AskUserQuestion (PermissionRequest without suggestions) needs standard hints.
    if req.hook_event_name != HookEventType::PermissionRequest
        || req.permission_suggestions.is_none()
    {
        push_action_hints(&mut lines, req.hook_event_name, req);
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

/// Check if tool_input contains AskUserQuestion-style questions.
fn has_ask_questions(tool_input: &serde_json::Value) -> bool {
    tool_input
        .get("questions")
        .and_then(|v| v.as_array())
        .map_or(false, |a| !a.is_empty())
}

fn push_ask_question_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest) {
    let questions = match req.tool_input.get("questions").and_then(|v| v.as_array()) {
        Some(q) => q,
        None => return,
    };

    let header_style = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);
    let question_style = Style::default().fg(Color::White);
    let option_idx_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let option_label_style = Style::default().fg(Color::Yellow);
    let option_desc_style = Style::default().fg(Color::DarkGray);

    for (qi, q) in questions.iter().enumerate() {
        if qi > 0 {
            lines.push(Line::from(""));
        }

        // Header tag
        if let Some(header) = q.get("header").and_then(|v| v.as_str()) {
            lines.push(Line::from(Span::styled(
                format!("  [{header}]"),
                header_style,
            )));
        }

        // Question text
        if let Some(question) = q.get("question").and_then(|v| v.as_str()) {
            push_section_label(lines, "Question");
            lines.push(Line::from(""));
            for line in question.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    question_style,
                )));
            }
        }

        // Options
        if let Some(options) = q.get("options").and_then(|v| v.as_array()) {
            lines.push(Line::from(""));
            push_section_label(lines, "Options");
            lines.push(Line::from(""));

            for (i, opt) in options.iter().enumerate() {
                let label = opt.get("label").and_then(|v| v.as_str()).unwrap_or("?");
                let desc = opt.get("description").and_then(|v| v.as_str());

                lines.push(Line::from(vec![
                    Span::styled(format!("  [{}] ", i + 1), option_idx_style),
                    Span::styled(label.to_string(), option_label_style),
                ]));
                if let Some(d) = desc {
                    lines.push(Line::from(Span::styled(
                        format!("      {d}"),
                        option_desc_style,
                    )));
                }
            }
            // "Other" option — opens text input
            lines.push(Line::from(vec![
                Span::styled("  [O] ", option_idx_style),
                Span::styled("Other (type response)", Style::default().fg(Color::DarkGray)),
            ]));
        }

        // Multi-select indicator
        if q.get("multiSelect").and_then(|v| v.as_bool()).unwrap_or(false) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  (multi-select enabled)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
}

/// Check if event_data contains plan content (ExitPlanMode).
fn has_plan_content(req: &DecisionRequest) -> bool {
    req.event_data
        .as_ref()
        .and_then(|d| d.get("plan_content"))
        .and_then(|v| v.as_str())
        .map_or(false, |s| !s.is_empty())
}

fn push_plan_detail(lines: &mut Vec<Line<'static>>, req: &DecisionRequest, markdown_preview: bool) {
    push_section_label(lines, "Plan");
    lines.push(Line::from(""));

    if let Some(plan) = req
        .event_data
        .as_ref()
        .and_then(|d| d.get("plan_content"))
        .and_then(|v| v.as_str())
    {
        if markdown_preview {
            push_markdown_lines(lines, plan);
        } else {
            for line in plan.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::White),
                )));
            }
        }
    }
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

fn push_action_hints(lines: &mut Vec<Line<'static>>, event_type: wisphive_protocol::HookEventType, req: &DecisionRequest) {
    use wisphive_protocol::HookEventType;

    lines.push(Line::from(""));
    push_section_label(lines, "Actions");
    lines.push(Line::from(""));

    let hint_style = Style::default().fg(Color::DarkGray);
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let is_plan = has_plan_content(req);

    // AskUserQuestion: either PermissionRequest without suggestions, or PreToolUse with tool_name
    let is_ask_question = !is_plan
        && ((event_type == HookEventType::PermissionRequest
            && req.permission_suggestions.is_none())
        || (req.tool_name.eq_ignore_ascii_case("askuserquestion")
            && has_ask_questions(&req.tool_input)));

    let option_count = if is_ask_question {
        req.tool_input
            .get("questions")
            .and_then(|v| v.as_array())
            .and_then(|qs| qs.first())
            .and_then(|q| q.get("options"))
            .and_then(|v| v.as_array())
            .map_or(0, |o| o.len())
    } else {
        0
    };

    // For AskUserQuestion, render the dynamic "1-N" hint first
    if is_ask_question && option_count > 0 {
        let key_label = format!("[1-{}]", option_count);
        lines.push(Line::from(vec![
            Span::styled("  ", hint_style),
            Span::styled(key_label, key_style),
            Span::styled(" select an option", hint_style),
        ]));
    }

    let actions: Vec<(&str, &str)> = if is_plan {
        vec![
            ("A", "accept plan (exit plan mode)"),
            ("D", "reject (continue planning)"),
            ("M", "reject with feedback"),
        ]
    } else if is_ask_question {
        vec![
            ("O", "type custom response"),
            ("D", "deny"),
            ("M", "deny with message"),
        ]
    } else {
        match event_type {
            HookEventType::Stop | HookEventType::SubagentStop => vec![
                ("A/Enter", "accept (let agent stop)"),
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
        }
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

/// Render markdown text as styled ratatui Lines.
fn push_markdown_lines(lines: &mut Vec<Line<'static>>, text: &str) {
    let parser = Parser::new(text);

    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default().fg(Color::White);
    let mut in_code_block = false;
    let mut list_depth: usize = 0;
    let mut style_depth: Vec<Style> = Vec::new();

    for event in parser {
        match event {
            MdEvent::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    style_depth.push(current_style);
                    current_style = match level {
                        pulldown_cmark::HeadingLevel::H1 => Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        pulldown_cmark::HeadingLevel::H2 => Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                        _ => Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                    };
                }
                Tag::Strong => {
                    style_depth.push(current_style);
                    current_style = current_style.add_modifier(Modifier::BOLD);
                }
                Tag::Emphasis => {
                    style_depth.push(current_style);
                    current_style = current_style.add_modifier(Modifier::ITALIC);
                }
                Tag::CodeBlock(_) => {
                    in_code_block = true;
                    lines.push(Line::from(""));
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    let indent = "  ".repeat(list_depth);
                    current_spans.push(Span::styled(
                        format!("  {indent}• "),
                        Style::default().fg(Color::Cyan),
                    ));
                }
                Tag::Paragraph => {}
                _ => {}
            },
            MdEvent::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    current_style = style_depth.pop().unwrap_or(Style::default().fg(Color::White));
                    flush_spans(lines, &mut current_spans);
                    lines.push(Line::from(""));
                }
                TagEnd::Strong | TagEnd::Emphasis => {
                    current_style = style_depth.pop().unwrap_or(Style::default().fg(Color::White));
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    lines.push(Line::from(""));
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                }
                TagEnd::Item => {
                    flush_spans(lines, &mut current_spans);
                }
                TagEnd::Paragraph => {
                    flush_spans(lines, &mut current_spans);
                    lines.push(Line::from(""));
                }
                _ => {}
            },
            MdEvent::Text(text) => {
                if in_code_block {
                    let code_style = Style::default().fg(Color::Yellow);
                    for line in text.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("    {line}"),
                            code_style,
                        )));
                    }
                } else {
                    current_spans.push(Span::styled(
                        text.to_string(),
                        current_style,
                    ));
                }
            }
            MdEvent::Code(code) => {
                current_spans.push(Span::styled(
                    format!("`{code}`"),
                    Style::default().fg(Color::Yellow),
                ));
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => {
                flush_spans(lines, &mut current_spans);
            }
            MdEvent::Rule => {
                lines.push(Line::from(Span::styled(
                    "  ────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }
    flush_spans(lines, &mut current_spans);
}

/// Flush accumulated spans into a Line, prepending indent.
fn flush_spans(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if spans.is_empty() {
        return;
    }
    let mut line_spans = vec![Span::raw("  ")];
    line_spans.append(spans);
    lines.push(Line::from(line_spans));
}
