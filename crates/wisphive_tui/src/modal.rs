use uuid::Uuid;

/// What action the modal is confirming.
pub enum ModalAction {
    ApproveSingle(Uuid),
    DenySingle(Uuid),
    ApproveAll,
    DenyAll,
}

/// A confirmation dialog.
pub struct Modal {
    pub title: String,
    pub body: String,
    pub action: ModalAction,
}

impl Modal {
    pub fn confirm_approve(id: Uuid, tool_name: &str, agent_id: &str) -> Self {
        Self {
            title: "Confirm Approve".into(),
            body: format!("Approve {tool_name} from {agent_id}? [y/N]"),
            action: ModalAction::ApproveSingle(id),
        }
    }

    pub fn confirm_deny(id: Uuid, tool_name: &str, agent_id: &str) -> Self {
        Self {
            title: "Confirm Deny".into(),
            body: format!("Deny {tool_name} from {agent_id}? This will block the tool call. [y/N]"),
            action: ModalAction::DenySingle(id),
        }
    }

    pub fn confirm_approve_all(count: usize) -> Self {
        Self {
            title: "Confirm Approve All".into(),
            body: format!("Approve all {count} pending items? [y/N]"),
            action: ModalAction::ApproveAll,
        }
    }

    pub fn confirm_deny_all(count: usize) -> Self {
        Self {
            title: "Confirm Deny All".into(),
            body: format!("Deny all {count} pending items? This will block all tool calls. [y/N]"),
            action: ModalAction::DenyAll,
        }
    }
}
