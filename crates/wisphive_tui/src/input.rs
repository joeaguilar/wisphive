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
        ViewMode::Dashboard => {}
    }

    // If in filter input mode, handle text input
    if app.filter_input_mode {
        return handle_filter_input(app, key);
    }

    // Global keybindings
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => return InputAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return InputAction::Quit;
        }
        // Open history view (global — works from any panel)
        KeyCode::Char('h') => {
            app.enter_history_view(None);
            return InputAction::QueryHistory { agent_id: None };
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
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => InputAction::Quit,
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
        KeyCode::Esc => {
            app.exit_history_view();
            InputAction::None
        }
        KeyCode::Char('q') => InputAction::Quit,
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
        // Start search mode
        KeyCode::Char('/') => {
            app.history_search_mode = true;
            app.history_search_buffer.clear();
            InputAction::None
        }
        // Clear search
        KeyCode::Char('C') => {
            app.history_search_query = None;
            app.enter_history_view(app.history_agent_filter.clone());
            InputAction::QueryHistory {
                agent_id: app.history_agent_filter.clone(),
            }
        }
        // Filter history to selected entry's agent
        KeyCode::Char('f') => {
            if let Some(entry) = app.history.get(app.history_index) {
                let agent_id = entry.agent_id.clone();
                app.enter_history_view(Some(agent_id.clone()));
                return InputAction::QueryHistory {
                    agent_id: Some(agent_id),
                };
            }
            InputAction::None
        }
        // Clear agent filter (show all)
        KeyCode::Char('F') => {
            app.enter_history_view(None);
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
        KeyCode::Char('q') | KeyCode::Char('Q') => InputAction::Quit,
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
