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
            body: format!(
                "Approve {tool_name} from {agent_id}?\n\n  Y = approve  |  N / Esc = cancel"
            ),
            action: ModalAction::ApproveSingle(id),
        }
    }

    pub fn confirm_deny(id: Uuid, tool_name: &str, agent_id: &str) -> Self {
        Self {
            title: "Confirm Deny".into(),
            body: format!(
                "Deny {tool_name} from {agent_id}?\nThis will block the tool call.\n\n  Y = deny  |  N / Esc = cancel"
            ),
            action: ModalAction::DenySingle(id),
        }
    }

    pub fn confirm_approve_all(count: usize) -> Self {
        Self {
            title: "Confirm Approve All".into(),
            body: format!(
                "Approve all {count} pending items?\n\n  Y = approve all  |  N / Esc = cancel"
            ),
            action: ModalAction::ApproveAll,
        }
    }

    pub fn confirm_deny_all(count: usize) -> Self {
        Self {
            title: "Confirm Deny All".into(),
            body: format!(
                "Deny all {count} pending items?\nThis will block all tool calls.\n\n  Y = deny all  |  N / Esc = cancel"
            ),
            action: ModalAction::DenyAll,
        }
    }
}
