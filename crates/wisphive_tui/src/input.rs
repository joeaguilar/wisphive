use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use crate::app::{App, FocusPanel, ViewMode};
use crate::modal::{Modal, ModalAction};

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
    /// Request project summaries from the daemon.
    QueryProjects,
    /// Approve a PermissionRequest with a specific suggestion selected.
    ApprovePermission { id: uuid::Uuid, suggestion_index: usize },
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
        ViewMode::ProjectsExplorer => return handle_projects_view_input(app, key),
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
        // Open projects view
        KeyCode::Char('p') => {
            app.enter_projects_view();
            return InputAction::QueryProjects;
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

        // Spawn a new agent
        KeyCode::Char('n') => {
            app.modal = Some(Modal::spawn_agent());
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
    use wisphive_protocol::HookEventType;

    // Common keys: scroll, back, quit
    match key.code {
        KeyCode::Esc => { app.exit_detail_view(); return InputAction::None; }
        KeyCode::Char('q') => { app.exit_detail_view(); return InputAction::None; }
        KeyCode::Char('Q') => return InputAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => { app.detail_scroll = app.detail_scroll.saturating_add(1); return InputAction::None; }
        KeyCode::Char('k') | KeyCode::Up => { app.detail_scroll = app.detail_scroll.saturating_sub(1); return InputAction::None; }
        KeyCode::PageDown | KeyCode::Char(' ') => { app.detail_scroll = app.detail_scroll.saturating_add(20); return InputAction::None; }
        KeyCode::PageUp => { app.detail_scroll = app.detail_scroll.saturating_sub(20); return InputAction::None; }
        KeyCode::Char('g') => { app.detail_scroll = 0; return InputAction::None; }
        KeyCode::Char('G') => { app.detail_scroll = usize::MAX / 2; return InputAction::None; }
        _ => {}
    }

    // Event-specific keys
    let event_type = app.detail_event_type();
    match event_type {
        HookEventType::PermissionRequest => handle_permission_request_keys(app, key),
        HookEventType::Stop | HookEventType::SubagentStop => handle_stop_keys(app, key),
        HookEventType::UserPromptSubmit | HookEventType::ConfigChange => handle_binary_block_keys(app, key),
        HookEventType::Elicitation => handle_elicitation_keys(app, key),
        HookEventType::TeammateIdle => handle_teammate_idle_keys(app, key),
        HookEventType::TaskCompleted => handle_task_completed_keys(app, key),
        _ => handle_pre_tool_use_keys(app, key), // PreToolUse + fallback
    }
}

/// PreToolUse: Y=approve, N=deny, M=deny+msg, !=always, E=edit, C=context, ?=defer
fn handle_pre_tool_use_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            InputAction::None
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Deny(id);
            }
            InputAction::None
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
        KeyCode::Char('!') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::confirm_always_allow(req.id, &req.tool_name));
            }
            InputAction::None
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::edit_input(req.id, &req.tool_input));
            }
            InputAction::None
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::approve_with_context(req.id));
            }
            InputAction::None
        }
        KeyCode::Char('?') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::confirm_ask_defer(req.id));
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// PermissionRequest: 1-9=select suggestion, N=deny, M=deny+msg, ?=defer
fn handle_permission_request_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char(c @ '1'..='9') => {
            if let Some(req) = app.detail_request() {
                let idx = (c as usize) - ('1' as usize);
                let valid = req.permission_suggestions.as_ref().map_or(false, |s| idx < s.len());
                if valid {
                    let id = req.id;
                    app.exit_detail_view();
                    return InputAction::ApprovePermission { id, suggestion_index: idx };
                }
            }
            InputAction::None
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Deny(id);
            }
            InputAction::None
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
        KeyCode::Char('?') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::confirm_ask_defer(req.id));
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// Stop/SubagentStop: A=accept (approve = let stop)
fn handle_stop_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Enter => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// UserPromptSubmit/ConfigChange: A=allow, B=block, M=block+reason
fn handle_binary_block_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            InputAction::None
        }
        KeyCode::Char('b') | KeyCode::Char('B') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Deny(id);
            }
            InputAction::None
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            if let Some(req) = app.detail_request() {
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// Elicitation: A=accept (opens edit modal for content), D=decline, C=cancel
fn handle_elicitation_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if let Some(req) = app.detail_request() {
                // Pre-fill with schema template if available
                let initial = req.event_data
                    .as_ref()
                    .and_then(|d| d.get("requested_schema"))
                    .map(|s| serde_json::to_string_pretty(s).unwrap_or_default())
                    .unwrap_or_else(|| "{}".into());
                app.modal = Some(Modal::edit_input(req.id, &serde_json::json!(initial)));
            }
            InputAction::None
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Deny(id);
            }
            InputAction::None
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::DenyWithMessage { id, message: "cancel".into() };
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// TeammateIdle: C=continue+feedback (text modal), S=stop teammate
fn handle_teammate_idle_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(req) = app.detail_request() {
                // "Deny" with message = continue with feedback
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            InputAction::None
        }
        _ => InputAction::None,
    }
}

/// TaskCompleted: A=accept, R=reject+feedback (text modal)
fn handle_task_completed_keys(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if let Some(req) = app.detail_request() {
                let id = req.id;
                app.exit_detail_view();
                return InputAction::Approve(id);
            }
            InputAction::None
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if let Some(req) = app.detail_request() {
                // "Deny" with message = reject with feedback
                app.modal = Some(Modal::deny_with_message(req.id));
            }
            InputAction::None
        }
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

    // TextArea-based input modals (deny-with-message, approve-with-context, edit-input)
    if modal.textarea.is_some() {
        return handle_textarea_modal(app, modal, key);
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

fn handle_textarea_modal(app: &mut App, mut modal: Modal, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Esc => InputAction::None, // dismiss modal
        KeyCode::Enter => {
            let ta = modal.textarea.as_ref().unwrap();
            let buf = ta.lines().join("\n");
            let target_id = modal.target_id;

            match modal.action {
                ModalAction::DenyWithMessage => {
                    if buf.is_empty() {
                        app.modal = Some(modal);
                        return InputAction::None;
                    }
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::DenyWithMessage { id, message: buf }
                    } else {
                        InputAction::None
                    }
                }
                ModalAction::ApproveWithContext => {
                    if buf.is_empty() {
                        app.modal = Some(modal);
                        return InputAction::None;
                    }
                    if let Some(id) = target_id {
                        app.exit_detail_view();
                        InputAction::ApproveWithContext { id, context: buf }
                    } else {
                        InputAction::None
                    }
                }
                ModalAction::EditInput => {
                    if let Some(id) = target_id {
                        // Try to parse as JSON; if it looks like a bare command, wrap it
                        let updated = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&buf) {
                            val
                        } else {
                            serde_json::json!({ "command": buf })
                        };
                        app.exit_detail_view();
                        InputAction::ApproveWithInput { id, updated_input: updated }
                    } else {
                        InputAction::None
                    }
                }
                _ => InputAction::None,
            }
        }
        _ => {
            // Block Enter/newline insertion (Ctrl+M), delegate everything else to TextArea
            let input: tui_textarea::Input = key.into();
            if input.key == tui_textarea::Key::Char('m') && input.ctrl {
                // Block Ctrl+M (Enter alias) from inserting newline
            } else {
                modal.textarea.as_mut().unwrap().input(input);
            }
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

fn handle_projects_view_input(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.exit_projects_view();
            InputAction::None
        }
        KeyCode::Char('Q') => InputAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => {
            app.projects_down();
            InputAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.projects_up();
            InputAction::None
        }
        // Drill into project activity (search history by project path)
        KeyCode::Enter => {
            if let Some(project) = app.selected_project_summary() {
                let project_path = project.project.to_string_lossy().to_string();
                app.enter_history_view(None);
                return InputAction::SearchHistory {
                    search: wisphive_protocol::HistorySearch {
                        query: Some(project_path),
                        ..Default::default()
                    },
                };
            }
            InputAction::None
        }
        // Spawn agent in selected project
        KeyCode::Char('n') => {
            if let Some(project) = app.selected_project_summary() {
                let mut modal = Modal::spawn_agent();
                if let Some(ref mut spawn) = modal.spawn {
                    spawn.set_project(&project.project.to_string_lossy());
                }
                app.modal = Some(modal);
            }
            InputAction::None
        }
        // Refresh
        KeyCode::Char('r') => InputAction::QueryProjects,
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
        KeyCode::Tab | KeyCode::BackTab => {
            spawn.active_field = spawn.active_field.next();
            spawn.update_focus_styles();
            app.modal = Some(modal);
            InputAction::None
        }
        KeyCode::Esc => {
            // Dismiss
            InputAction::None
        }
        KeyCode::Enter => {
            let project = spawn.project_path();
            let prompt = spawn.prompt.lines()[0].clone();
            if prompt.is_empty() {
                app.modal = Some(modal);
                return InputAction::None;
            }
            InputAction::SpawnAgent { project, prompt }
        }
        _ => {
            // Block Enter/newline (Ctrl+M) from inserting, delegate rest to active TextArea
            let input: tui_textarea::Input = key.into();
            if input.key == tui_textarea::Key::Char('m') && input.ctrl {
                // Block Ctrl+M
            } else {
                spawn.active_textarea().input(input);
            }
            app.modal = Some(modal);
            InputAction::None
        }
    }
}
