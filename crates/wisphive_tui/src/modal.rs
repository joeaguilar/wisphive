use std::path::PathBuf;

use uuid::Uuid;

/// What action the modal is confirming.
pub enum ModalAction {
    ApproveAll,
    DenyAll,
    SpawnAgent,
    DenyWithMessage,
    ApproveWithContext,
    EditInput,
    AlwaysAllow,
    AskDefer,
}

/// Active input field in the spawn agent modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnField {
    Project,
    Prompt,
}

impl SpawnField {
    pub fn next(self) -> Self {
        match self {
            Self::Project => Self::Prompt,
            Self::Prompt => Self::Project,
        }
    }
}

/// State for the spawn-agent modal.
pub struct SpawnModal {
    pub project_buf: String,
    pub prompt_buf: String,
    pub active_field: SpawnField,
}

impl SpawnModal {
    pub fn new() -> Self {
        let project = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            project_buf: project,
            prompt_buf: String::new(),
            active_field: SpawnField::Prompt,
        }
    }

    pub fn project_path(&self) -> PathBuf {
        PathBuf::from(&self.project_buf)
    }
}

impl Default for SpawnModal {
    fn default() -> Self {
        Self::new()
    }
}

/// State for a single-field text input modal.
pub struct TextInputModal {
    pub buffer: String,
}

impl TextInputModal {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }
}

impl Default for TextInputModal {
    fn default() -> Self {
        Self::new()
    }
}

/// State for editing tool input (pre-filled with current value).
pub struct EditInputModal {
    pub buffer: String,
}

impl EditInputModal {
    pub fn new(initial: &str) -> Self {
        Self {
            buffer: initial.to_string(),
        }
    }
}

/// A confirmation or input dialog.
pub struct Modal {
    pub title: String,
    pub body: String,
    pub action: ModalAction,
    /// The decision request this modal acts on.
    pub target_id: Option<Uuid>,
    /// State for the spawn-agent modal.
    pub spawn: Option<SpawnModal>,
    /// State for text input modals (deny-with-message, approve-with-context).
    pub text_input: Option<TextInputModal>,
    /// State for the edit-input modal.
    pub edit_input: Option<EditInputModal>,
}

impl Modal {
    pub fn confirm_approve_all(count: usize) -> Self {
        Self {
            title: "Confirm Approve All".into(),
            body: format!(
                "Approve all {count} pending items?\n\n  Y = approve all  |  N / Esc = cancel"
            ),
            action: ModalAction::ApproveAll,
            target_id: None,
            spawn: None,
            text_input: None,
            edit_input: None,
        }
    }

    pub fn confirm_deny_all(count: usize) -> Self {
        Self {
            title: "Confirm Deny All".into(),
            body: format!(
                "Deny all {count} pending items?\nThis will block all tool calls.\n\n  Y = deny all  |  N / Esc = cancel"
            ),
            action: ModalAction::DenyAll,
            target_id: None,
            spawn: None,
            text_input: None,
            edit_input: None,
        }
    }

    pub fn spawn_agent() -> Self {
        Self {
            title: "Spawn Agent".into(),
            body: "Tab to switch fields, Enter to spawn, Esc to cancel".into(),
            action: ModalAction::SpawnAgent,
            target_id: None,
            spawn: Some(SpawnModal::new()),
            text_input: None,
            edit_input: None,
        }
    }

    pub fn deny_with_message(id: Uuid) -> Self {
        Self {
            title: "Deny with Message".into(),
            body: "Type a reason (Claude will see this as feedback):".into(),
            action: ModalAction::DenyWithMessage,
            target_id: Some(id),
            spawn: None,
            text_input: Some(TextInputModal::new()),
            edit_input: None,
        }
    }

    pub fn approve_with_context(id: Uuid) -> Self {
        Self {
            title: "Approve with Context".into(),
            body: "Type additional context (injected into Claude's conversation):".into(),
            action: ModalAction::ApproveWithContext,
            target_id: Some(id),
            spawn: None,
            text_input: Some(TextInputModal::new()),
            edit_input: None,
        }
    }

    pub fn edit_input(id: Uuid, tool_input: &serde_json::Value) -> Self {
        // For Bash, show just the command; otherwise show pretty JSON
        let initial = if let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) {
            cmd.to_string()
        } else {
            serde_json::to_string_pretty(tool_input).unwrap_or_default()
        };
        Self {
            title: "Edit Input".into(),
            body: "Modify the input, Enter to approve with changes, Esc to cancel:".into(),
            action: ModalAction::EditInput,
            target_id: Some(id),
            spawn: None,
            text_input: None,
            edit_input: Some(EditInputModal::new(&initial)),
        }
    }

    pub fn confirm_always_allow(id: Uuid, tool_name: &str) -> Self {
        Self {
            title: "Always Allow".into(),
            body: format!(
                "Always allow \"{tool_name}\"?\nThis adds it to ~/.wisphive/auto-approve.json\n\n  Y = always allow  |  N / Esc = cancel"
            ),
            action: ModalAction::AlwaysAllow,
            target_id: Some(id),
            spawn: None,
            text_input: None,
            edit_input: None,
        }
    }

    pub fn confirm_ask_defer(id: Uuid) -> Self {
        Self {
            title: "Defer".into(),
            body: "Pass to Claude's native permission prompt?\n\n  Y = defer  |  N / Esc = cancel".into(),
            action: ModalAction::AskDefer,
            target_id: Some(id),
            spawn: None,
            text_input: None,
            edit_input: None,
        }
    }
}
