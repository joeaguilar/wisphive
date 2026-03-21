use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use crate::app::{App, FocusPanel, ViewMode};
use crate::modal::{Modal, ModalAction, SpawnField};

/// Action the main loop should take after processing input.
pub enum InputAction {
    /// No action needed.
    None,
    /// Approve the decision with the given UUID.
    Approve(uuid::Uuid),
    /// Deny the decision with the given UUID.
    Deny(uuid::Uuid),
    /// Approve all (optionally filtered).
    ApproveAll,
    /// Deny all (optionally filtered).
    DenyAll,
    /// Spawn a new agent with the given project and prompt.
    SpawnAgent { project: PathBuf, prompt: String },
    /// Request history from the daemon (optional agent_id filter).
    QueryHistory { agent_id: Option<String> },
    /// Search history with a query string.
    SearchHistory { search: wisphive_protocol::HistorySearch },
    /// Request a specific page of history.
    QueryHistoryPage { agent_id: Option<String>, page: usize },
    /// Deny with a feedback message for the agent.
    DenyWithMessage { id: uuid::Uuid, message: String },
    /// Approve and add tool to always-allow list.
    AlwaysAllow(uuid::Uuid),
    /// Approve with modified tool input.
    ApproveWithInput { id: uuid::Uuid, updated_input: serde_json::Value },
    /// Approve with additional context injected into the agent.
    ApproveWithContext { id: uuid::Uuid, context: String },
    /// Defer to agent's native permission prompt.
    AskDefer(uuid::Uuid),
    /// Request session summaries from the daemon.
    QuerySessions,
    /// Request the timeline for a specific session.
    QuerySessionTimeline { agent_id: String },
    /// Request a specific page of session timeline.
    QuerySessionTimelinePage { agent_id: String, page: usize },
    /// Quit the application.
    Quit,
}

/// Process a crossterm event and update app state.
pub fn handle_event(app: &mut App, event: Event) -> InputAction {
    let Event::Key(key) = event else {
        return InputAction::None;
    };

    // If a modal is active, handle it first
    if app.modal.is_some() {
        return handle_modal_input(app, key);
    }

    // Route based on view mode
    match app.view_mode {
        ViewMode::Detail => return handle_detail_input(app, key),
        ViewMode::History => return handle_history_input(app, key),
        ViewMode::HistoryDetail => return handle_history_detail_input(app, key),
        ViewMode::Config => return handle_config_input(app, key),
        ViewMode::Sessions => return handle_sessions_input(app, key),
        ViewMode::SessionTimeline => return handle_session_timeline_input(app, key),
        ViewMode::Dashboard => {}
    }

    // If in filter input mode, handle text input
    if app.filter_input_mode {
        return handle_filter_input(app, key);
    }

    // Global keybindings
    match key.code {
        KeyCode::Char('Q') => return InputAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return InputAction::Quit;
        }
        // Navigate back (q) / forward (e)
        KeyCode::Char('q') => {
            if !app.navigate_back() {
                return InputAction::Quit; // No history = quit from dashboard
            }
            return InputAction::None;
        }
        KeyCode::Char('e') => {
            app.navigate_forward();
            return InputAction::None;
        }
        // Open history view (global — works from any panel)
        KeyCode::Char('h') => {
            app.enter_history_view(None);
            return InputAction::QueryHistory { agent_id: None };
        }
        // Open sessions view
        KeyCode::Char('s') => {
            app.enter_sessions_view();
            return InputAction::QuerySessions;
        }
        // Open config view
        KeyCode::Char('c') => {
            app.enter_config_view();
            return InputAction::None;
        }
        _ => {}
    }

    // Panel-specific keybindings
    match app.focus {
        FocusPanel::Queue => handle_queue_input(app, key),
        FocusPanel::Agents | FocusPanel::Projects => {
            match key.code {
                KeyCode::Tab => {
                    app.cycle_focus();
                    InputAction::None
                }
                // Spawn a new agent
                KeyCode::Char('n') => {
                    app.modal = Some(Modal::spawn_agent());
                    InputAction::None
                }
                _ => InputAction::None,
            }
        }
    }
}

fn handle_queue_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => {
            app.queue_down();
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.queue_up();
            InputAction::None
        }
        KeyCode::Tab => {
            app.cycle_focus();
            InputAction::None
        }

        // Quick approve selected (no detail view)
        KeyCode::Char('y') => {
            if let Some(req) = app.selected_request() {
                return InputAction::Approve(req.id);
            }
            InputAction::None
        }

        // Open detail view for review (approve/deny from there)
        KeyCode::Char('a') | KeyCode::Char('d') => {
            app.enter_detail_view();
            InputAction::None
        }

        // Approve all
        KeyCode::Char('A') => {
            let count = app.filtered_queue().len();
            if count > 0 {
                app.modal = Some(Modal::confirm_approve_all(count));
            }
            InputAction::None
        }

        // Deny all
        KeyCode::Char('D') => {
            let count = app.filtered_queue().len();
            if count > 0 {
                app.modal = Some(Modal::confirm_deny_all(count));
            }
            InputAction::None
        }

        // Filter
        KeyCode::Char('/') => {
            app.filter_input_mode = true;
            app.filter_buffer.clear();
            InputAction::None
        }

        // Clear filter
        KeyCode::Esc => {
            app.filter = None;
            app.queue_index = 0;
            InputAction::None
        }

        // Open detail view for the selected item
        KeyCode::Enter => {
            app.enter_detail_view();
            InputAction::None
        }

        _ => InputAction::None,
    }
}

fn handle_detail_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        // Approve
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            app.exit_detail_view();
            InputAction::None
        }
        // Deny (simple)
        KeyCode::Char('n') | KeyCode::Char('N') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Deny(id);
            }
            app.exit_detail_view();
            InputAction::None
        }
        // Deny with message
        KeyCode::Char('m') | KeyCode::Char('M') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
        // Always allow this tool
        KeyCode::Char('!') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::confirm_always_allow(req.id, &req.tool_name));
            }
            InputAction::None
        }
        // Edit input before approving
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::edit_input(req.id, &req.tool_input));
            }
            InputAction::None
        }
        // Approve with additional context
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::approve_with_context(req.id));
            }
            InputAction::None
        }
        // Ask/defer to native prompt
        KeyCode::Char('?') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::confirm_ask_defer(req.id));
            }
            InputAction::None
        }
        // Back to dashboard
        KeyCode::Esc => {
            app.exit_detail_view();
            InputAction::None
        }
        // Scroll down
        KeyCode::Char('j') | KeyCode::Down => {
            app.detail_scroll = app.detail_scroll.saturating_add(1);
            InputAction::None
        }
        // Scroll up
        KeyCode::Char('k') | KeyCode::Up => {
            app.detail_scroll = app.detail_scroll.saturating_sub(1);
            InputAction::None
        }
        // Page down
        KeyCode::PageDown | KeyCode::Char(' ') => {
            app.detail_scroll = app.detail_scroll.saturating_add(20);
            InputAction::None
        }
        // Page up
        KeyCode::PageUp => {
            app.detail_scroll = app.detail_scroll.saturating_sub(20);
            InputAction::None
        }
        // Jump to top
        KeyCode::Char('g') => {
            app.detail_scroll = 0;
            InputAction::None
        }
        // Jump to bottom
        KeyCode::Char('G') => {
            app.detail_scroll = usize::MAX / 2;
            InputAction::None
        }
        // Navigate back
        KeyCode::Char('q') => {
            app.exit_detail_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        _ => InputAction::None,
    }
}

fn handle_filter_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Enter => {
            app.filter_input_mode = false;
            if app.filter_buffer.is_empty() {
                app.filter = None;
            } else {
                app.filter = Some(app.filter_buffer.clone());
            }
            app.queue_index = 0;
            InputAction::None
        }
        KeyCode::Esc => {
            app.filter_input_mode = false;
            app.filter_buffer.clear();
            InputAction::None
        }
        KeyCode::Backspace => {
            app.filter_buffer.pop();
            InputAction::None
        }
        KeyCode::Char(c) => {
            app.filter_buffer.push(c);
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_modal_input(app: &mut App, key: KeyEvent) -> InputAction {
    let modal = app.modal.take().unwrap();

    // Spawn modal has its own text-input handling
    if modal.spawn.is_some() {
        return handle_spawn_modal_input(app, modal, key);
    }

    // Text input modals (deny-with-message, approve-with-context)
    if modal.text_input.is_some() {
        return handle_text_input_modal(app, modal, key);
    }

    // Edit input modal
    if modal.edit_input.is_some() {
        return handle_edit_input_modal(app, modal, key);
    }

    // Simple Y/N confirmation modals
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let target_id = modal.target_id;
            match modal.action {
                ModalAction::ApproveAll => InputAction::ApproveAll,
                ModalAction::DenyAll => InputAction::DenyAll,
                ModalAction::AlwaysAllow => {
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::AlwaysAllow(id)
                    } else {
                        InputAction::None
                    }
                }
                ModalAction::AskDefer => {
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::AskDefer(id)
                    } else {
                        InputAction::None
                    }
                }
                _ => InputAction::None, // unreachable for non-confirm modals
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => InputAction::None,
        _ => {
            app.modal = Some(modal);
            InputAction::None
        }
    }
}

fn handle_text_input_modal(app: &mut App, mut modal: Modal, key: KeyEvent) -> InputAction {
    let text = modal.text_input.as_mut().unwrap();

    match key.code {
        KeyCode::Esc => InputAction::None,
        KeyCode::Enter => {
            let buf = text.buffer.clone();
            let target_id = modal.target_id;
            if buf.is_empty() {
                app.modal = Some(modal);
                return InputAction::None;
            }
            match modal.action {
                ModalAction::DenyWithMessage => {
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::DenyWithMessage { id, message: buf }
                    } else {
                        InputAction::None
                    }
                }
                ModalAction::ApproveWithContext => {
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::ApproveWithContext { id, context: buf }
                    } else {
                        InputAction::None
                    }
                }
                _ => InputAction::None,
            }
        }
        KeyCode::Backspace => {
            text.buffer.pop();
            app.modal = Some(modal);
            InputAction::None
        }
        KeyCode::Char(c) => {
            text.buffer.push(c);
            app.modal = Some(modal);
            InputAction::None
        }
        _ => {
            app.modal = Some(modal);
            InputAction::None
        }
    }
}

fn handle_edit_input_modal(app: &mut App, mut modal: Modal, key: KeyEvent) -> InputAction {
    let edit = modal.edit_input.as_mut().unwrap();

    match key.code {
        KeyCode::Esc => InputAction::None,
        KeyCode::Enter => {
            let buf = edit.buffer.clone();
            let target_id = modal.target_id;
            if let Some(id) = target_id {
                // Try to parse as JSON; if it looks like a bare command, wrap it
                let updated = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&buf) {
                    val
                } else {
                    // Treat as a Bash command string
                    serde_json::json!({ "command": buf })
                };
                app.exit_detail_view();
                InputAction::ApproveWithInput { id, updated_input: updated }
            } else {
                InputAction::None
            }
        }
        KeyCode::Backspace => {
            edit.buffer.pop();
            app.modal = Some(modal);
            InputAction::None
        }
        KeyCode::Char(c) => {
            edit.buffer.push(c);
            app.modal = Some(modal);
            InputAction::None
        }
        _ => {
            app.modal = Some(modal);
            InputAction::None
        }
    }
}

/// Handle input in the history view.
fn handle_history_input(app: &mut App, key: KeyEvent) -> InputAction {
    // If in search input mode, handle text input
    if app.history_search_mode {
        return handle_history_search_input(app, key);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.exit_history_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            app.history_down();
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.history_up();
            InputAction::None
        }
        // Open detail view for selected history entry
        KeyCode::Enter => {
            app.enter_history_detail_view();
            InputAction::None
        }
        // Paginate: previous page
        KeyCode::Char('[') | KeyCode::Char('H') => {
            if app.history_page > 0 {
                app.history_page -= 1;
                app.history_index = 0;
                return InputAction::QueryHistoryPage {
                    agent_id: app.history_agent_filter.clone(),
                    page: app.history_page,
                };
            }
            InputAction::None
        }
        // Paginate: next page
        KeyCode::Char(']') | KeyCode::Char('L') => {
            if app.history_has_more {
                app.history_page += 1;
                app.history_index = 0;
                return InputAction::QueryHistoryPage {
                    agent_id: app.history_agent_filter.clone(),
                    page: app.history_page,
                };
            }
            InputAction::None
        }
        // Start search mode
        KeyCode::Char('/') => {
            app.history_search_mode = true;
            app.history_search_buffer.clear();
            InputAction::None
        }
        // Clear search
        KeyCode::Char('C') => {
            app.history_search_query = None;
            app.history_page = 0;
            InputAction::QueryHistory {
                agent_id: app.history_agent_filter.clone(),
            }
        }
        // Filter history to selected entry's agent
        KeyCode::Char('f') => {
            if let Some(entry) = app.history.get(app.history_index) {
                let agent_id = entry.agent_id.clone();
                app.history_agent_filter = Some(agent_id.clone());
                app.history_page = 0;
                app.history_index = 0;
                return InputAction::QueryHistory {
                    agent_id: Some(agent_id),
                };
            }
            InputAction::None
        }
        // Clear agent filter (show all)
        KeyCode::Char('F') => {
            app.history_agent_filter = None;
            app.history_page = 0;
            app.history_index = 0;
            InputAction::QueryHistory { agent_id: None }
        }
        _ => InputAction::None,
    }
}

fn handle_history_search_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Enter => {
            app.history_search_mode = false;
            if app.history_search_buffer.is_empty() {
                app.history_search_query = None;
                return InputAction::QueryHistory {
                    agent_id: app.history_agent_filter.clone(),
                };
            }
            let query = app.history_search_buffer.clone();
            app.history_search_query = Some(query.clone());
            InputAction::SearchHistory {
                search: wisphive_protocol::HistorySearch {
                    query: Some(query),
                    agent_id: app.history_agent_filter.clone(),
                    ..Default::default()
                },
            }
        }
        KeyCode::Esc => {
            app.history_search_mode = false;
            app.history_search_buffer.clear();
            InputAction::None
        }
        KeyCode::Backspace => {
            app.history_search_buffer.pop();
            InputAction::None
        }
        KeyCode::Char(c) => {
            app.history_search_buffer.push(c);
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_history_detail_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Esc => {
            app.exit_history_detail_view();
            InputAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.detail_scroll = app.detail_scroll.saturating_add(1);
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.detail_scroll = app.detail_scroll.saturating_sub(1);
            InputAction::None
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            app.detail_scroll = app.detail_scroll.saturating_add(20);
            InputAction::None
        }
        KeyCode::PageUp => {
            app.detail_scroll = app.detail_scroll.saturating_sub(20);
            InputAction::None
        }
        KeyCode::Char('q') => {
            app.exit_history_detail_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        _ => InputAction::None,
    }
}

fn handle_config_input(app: &mut App, key: KeyEvent) -> InputAction {
    use crate::app::{ALL_TOOLS, ConfigRow};
    use wisphive_protocol::{AutoApproveLevel, ToolRule};

    // If in rule input mode, handle text input
    if app.config_rule_input_mode {
        return handle_config_rule_input(app, key);
    }

    let rows = app.config_rows();
    let max_idx = rows.len().saturating_sub(1);

    match key.code {
        // Back
        KeyCode::Esc | KeyCode::Char('q') => {
            app.exit_config_view();
            InputAction::None
        }
        // Navigate
        KeyCode::Char('j') | KeyCode::Down => {
            if app.config_index < max_idx {
                app.config_index += 1;
            }
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.config_index = app.config_index.saturating_sub(1);
            InputAction::None
        }
        // Cycle level (when on level row)
        KeyCode::Left | KeyCode::Right => {
            if let Some(ConfigRow::Level) = rows.get(app.config_index) {
                let levels = [
                    AutoApproveLevel::Off,
                    AutoApproveLevel::Read,
                    AutoApproveLevel::Write,
                    AutoApproveLevel::Execute,
                    AutoApproveLevel::All,
                ];
                let current = levels.iter().position(|l| *l == app.config_level).unwrap_or(1);
                let next = if key.code == KeyCode::Right {
                    (current + 1).min(levels.len() - 1)
                } else {
                    current.saturating_sub(1)
                };
                app.config_level = levels[next];
                app.save_config();
            }
            InputAction::None
        }
        // Toggle tool override (when on a tool row)
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(ConfigRow::Tool(tool_idx)) = rows.get(app.config_index) {
                let tool = ALL_TOOLS[*tool_idx].to_string();
                let in_level = app.config_level.includes(&tool);
                let in_add = app.config_add.contains(&tool);
                let in_remove = app.config_remove.contains(&tool);

                if in_level {
                    if in_remove {
                        app.config_remove.retain(|t| *t != tool);
                    } else {
                        app.config_add.retain(|t| *t != tool);
                        app.config_remove.push(tool);
                    }
                } else if in_add {
                    app.config_add.retain(|t| *t != tool);
                } else {
                    app.config_remove.retain(|t| *t != tool);
                    app.config_add.push(tool);
                }
                app.save_config();
            }
            InputAction::None
        }
        // Add rule (+) — on a tool row, start rule input
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if let Some(ConfigRow::Tool(tool_idx)) = rows.get(app.config_index) {
                let tool = ALL_TOOLS[*tool_idx];
                let in_remove = app.config_remove.iter().any(|t| t == tool);
                let in_add = app.config_add.iter().any(|t| t == tool);
                let in_level = app.config_level.includes(tool);
                // Auto-determine: deny if approved, allow if not
                let is_approved = !in_remove && (in_add || in_level);
                app.config_rule_input_mode = true;
                app.config_rule_buffer.clear();
                app.config_rule_target_tool = Some(tool.to_string());
                app.config_rule_is_deny = is_approved;
            }
            InputAction::None
        }
        // Remove rule (-) — on a rule row, delete it
        KeyCode::Char('-') => {
            if let Some(ConfigRow::Rule { tool_idx, rule_idx, is_deny }) = rows.get(app.config_index) {
                let tool = ALL_TOOLS[*tool_idx].to_string();
                let is_deny = *is_deny;
                let rule_idx = *rule_idx;
                let rule = app.config_tool_rules.entry(tool).or_insert_with(ToolRule::default);
                if is_deny {
                    if rule_idx < rule.deny_patterns.len() {
                        rule.deny_patterns.remove(rule_idx);
                    }
                } else if rule_idx < rule.allow_patterns.len() {
                    rule.allow_patterns.remove(rule_idx);
                }
                let new_rows = app.config_rows();
                if app.config_index >= new_rows.len() {
                    app.config_index = new_rows.len().saturating_sub(1);
                }
                app.save_config();
            }
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        _ => InputAction::None,
    }
}

fn handle_config_rule_input(app: &mut App, key: KeyEvent) -> InputAction {
    use wisphive_protocol::ToolRule;

    match key.code {
        KeyCode::Esc => {
            app.config_rule_input_mode = false;
            app.config_rule_buffer.clear();
            app.config_rule_target_tool = None;
            InputAction::None
        }
        KeyCode::Enter => {
            let pattern = app.config_rule_buffer.trim().to_string();
            if !pattern.is_empty() {
                if let Some(tool) = app.config_rule_target_tool.take() {
                    let rule = app.config_tool_rules.entry(tool).or_insert_with(ToolRule::default);
                    if app.config_rule_is_deny {
                        if !rule.deny_patterns.contains(&pattern) {
                            rule.deny_patterns.push(pattern);
                        }
                    } else if !rule.allow_patterns.contains(&pattern) {
                        rule.allow_patterns.push(pattern);
                    }
                    app.save_config();
                }
            }
            app.config_rule_input_mode = false;
            app.config_rule_buffer.clear();
            InputAction::None
        }
        KeyCode::Backspace => {
            app.config_rule_buffer.pop();
            InputAction::None
        }
        KeyCode::Char(c) => {
            app.config_rule_buffer.push(c);
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_sessions_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.exit_sessions_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            app.sessions_down();
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.sessions_up();
            InputAction::None
        }
        KeyCode::Enter => {
            if let Some(session) = app.selected_session() {
                let agent_id = session.agent_id.clone();
                app.enter_session_timeline_view(agent_id.clone());
                InputAction::QuerySessionTimeline { agent_id }
            } else {
                InputAction::None
            }
        }
        KeyCode::Char('r') => InputAction::QuerySessions,
        _ => InputAction::None,
    }
}

fn handle_session_timeline_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.exit_session_timeline_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            app.session_timeline_down();
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.session_timeline_up();
            InputAction::None
        }
        KeyCode::Enter => {
            app.enter_timeline_detail_view();
            InputAction::None
        }
        KeyCode::Char('[') | KeyCode::Char('H') => {
            if app.session_timeline_page > 0 {
                app.session_timeline_page -= 1;
                app.session_timeline_index = 0;
                let agent_id = app.session_timeline_agent_id.clone().unwrap_or_default();
                return InputAction::QuerySessionTimelinePage {
                    agent_id,
                    page: app.session_timeline_page,
                };
            }
            InputAction::None
        }
        KeyCode::Char(']') | KeyCode::Char('L') => {
            if app.session_timeline_has_more {
                app.session_timeline_page += 1;
                app.session_timeline_index = 0;
                let agent_id = app.session_timeline_agent_id.clone().unwrap_or_default();
                return InputAction::QuerySessionTimelinePage {
                    agent_id,
                    page: app.session_timeline_page,
                };
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_spawn_modal_input(
    app: &mut App,
    mut modal: Modal,
    key: KeyEvent,
) -> InputAction {
    let spawn = modal.spawn.as_mut().unwrap();

    match key.code {
        KeyCode::Tab => {
            spawn.active_field = spawn.active_field.next();
            app.modal = Some(modal);
            InputAction::None
        }
        KeyCode::Esc => {
            // Dismiss
            InputAction::None
        }
        KeyCode::Enter => {
            let project = spawn.project_path();
            let prompt = spawn.prompt_buf.clone();
            if prompt.is_empty() {
                // Don't submit with empty prompt — keep modal open
                app.modal = Some(modal);
                return InputAction::None;
            }
            InputAction::SpawnAgent { project, prompt }
        }
        KeyCode::Backspace => {
            match spawn.active_field {
                SpawnField::Project => { spawn.project_buf.pop(); }
                SpawnField::Prompt => { spawn.prompt_buf.pop(); }
            }
            app.modal = Some(modal);
            InputAction::None
        }
        KeyCode::Char(c) => {
            match spawn.active_field {
                SpawnField::Project => spawn.project_buf.push(c),
                SpawnField::Prompt => spawn.prompt_buf.push(c),
            }
            app.modal = Some(modal);
            InputAction::None
        }
        _ => {
            app.modal = Some(modal);
            InputAction::None
        }
    }
}
