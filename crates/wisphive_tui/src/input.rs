use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use crate::app::{App, FocusPanel};
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

        // Quick approve selected (no confirmation)
        KeyCode::Char('y') => {
            if let Some(req) = app.selected_request() {
                return InputAction::Approve(req.id);
            }
            InputAction::None
        }

        // Approve selected (with confirmation)
        KeyCode::Char('a') => {
            if let Some(req) = app.selected_request() {
                let id = req.id;
                app.modal = Some(Modal::confirm_approve(id, &req.tool_name, &req.agent_id));
                InputAction::None
            } else {
                InputAction::None
            }
        }

        // Deny selected
        KeyCode::Char('d') => {
            if let Some(req) = app.selected_request() {
                let id = req.id;
                app.modal = Some(Modal::confirm_deny(id, &req.tool_name, &req.agent_id));
                InputAction::None
            } else {
                InputAction::None
            }
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

        // Expand detail (future: toggle detail pane)
        KeyCode::Enter => InputAction::None,

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

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => match modal.action {
            ModalAction::ApproveSingle(id) => InputAction::Approve(id),
            ModalAction::DenySingle(id) => InputAction::Deny(id),
            ModalAction::ApproveAll => InputAction::ApproveAll,
            ModalAction::DenyAll => InputAction::DenyAll,
            ModalAction::SpawnAgent => InputAction::None, // unreachable
        },
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            // Modal dismissed, no action
            InputAction::None
        }
        _ => {
            // Unknown key while modal is open — keep the modal
            app.modal = Some(modal);
            InputAction::None
        }
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
