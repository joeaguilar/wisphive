use std::path::PathBuf;

use uuid::Uuid;

/// What action the modal is confirming.
pub enum ModalAction {
    ApproveSingle(Uuid),
    DenySingle(Uuid),
    ApproveAll,
    DenyAll,
    /// Spawn a new agent (resolved via SpawnModal fields).
    SpawnAgent,
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

/// A confirmation dialog.
pub struct Modal {
    pub title: String,
    pub body: String,
    pub action: ModalAction,
    /// State for the spawn-agent modal (only set when action is SpawnAgent).
    pub spawn: Option<SpawnModal>,
}

impl Modal {
    pub fn confirm_approve(id: Uuid, tool_name: &str, agent_id: &str) -> Self {
        Self {
            title: "Confirm Approve".into(),
            body: format!(
                "Approve {tool_name} from {agent_id}?\n\n  Y = approve  |  N / Esc = cancel"
            ),
            action: ModalAction::ApproveSingle(id),
            spawn: None,
        }
    }

    pub fn confirm_deny(id: Uuid, tool_name: &str, agent_id: &str) -> Self {
        Self {
            title: "Confirm Deny".into(),
            body: format!(
                "Deny {tool_name} from {agent_id}?\nThis will block the tool call.\n\n  Y = deny  |  N / Esc = cancel"
            ),
            action: ModalAction::DenySingle(id),
            spawn: None,
        }
    }

    pub fn confirm_approve_all(count: usize) -> Self {
        Self {
            title: "Confirm Approve All".into(),
            body: format!(
                "Approve all {count} pending items?\n\n  Y = approve all  |  N / Esc = cancel"
            ),
            action: ModalAction::ApproveAll,
            spawn: None,
        }
    }

    pub fn confirm_deny_all(count: usize) -> Self {
        Self {
            title: "Confirm Deny All".into(),
            body: format!(
                "Deny all {count} pending items?\nThis will block all tool calls.\n\n  Y = deny all  |  N / Esc = cancel"
            ),
            action: ModalAction::DenyAll,
            spawn: None,
        }
    }

    pub fn spawn_agent() -> Self {
        Self {
            title: "Spawn Agent".into(),
            body: "Tab to switch fields, Enter to spawn, Esc to cancel".into(),
            action: ModalAction::SpawnAgent,
            spawn: Some(SpawnModal::new()),
        }
    }
}
