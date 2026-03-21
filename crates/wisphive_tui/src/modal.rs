use std::path::PathBuf;

use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};
use tui_textarea::TextArea;
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

/// Create a single-line TextArea with consistent styling.
fn make_textarea(initial: &str, placeholder: &str) -> TextArea<'static> {
    let lines = if initial.is_empty() {
        vec![String::new()]
    } else {
        vec![initial.to_string()]
    };
    let mut ta = TextArea::new(lines);
    ta.set_cursor_line_style(Style::default());
    ta.set_style(Style::default().fg(Color::Yellow));
    ta.set_cursor_style(Style::default().fg(Color::Yellow).bg(Color::DarkGray));
    ta.set_placeholder_text(placeholder);
    ta.set_placeholder_style(Style::default().fg(Color::DarkGray));
    // Move cursor to end of initial text
    ta.move_cursor(tui_textarea::CursorMove::End);
    ta
}

/// State for the spawn-agent modal.
pub struct SpawnModal {
    pub project: TextArea<'static>,
    pub prompt: TextArea<'static>,
    pub active_field: SpawnField,
}

impl SpawnModal {
    pub fn new() -> Self {
        let project_path = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut project = make_textarea(&project_path, "/path/to/project");
        project.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Project "),
        );
        let mut prompt = make_textarea("", "Enter prompt for the agent...");
        prompt.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Prompt "),
        );
        Self {
            project,
            prompt,
            active_field: SpawnField::Prompt,
        }
    }

    pub fn project_path(&self) -> PathBuf {
        PathBuf::from(&self.project.lines()[0])
    }

    /// Replace the project field text (used when spawning from project view).
    pub fn set_project(&mut self, path: &str) {
        self.project = make_textarea(path, "/path/to/project");
        self.project.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Project "),
        );
    }

    pub fn active_textarea(&mut self) -> &mut TextArea<'static> {
        match self.active_field {
            SpawnField::Project => &mut self.project,
            SpawnField::Prompt => &mut self.prompt,
        }
    }

    /// Update border styles to reflect which field is active.
    pub fn update_focus_styles(&mut self) {
        let (proj_color, prompt_color) = match self.active_field {
            SpawnField::Project => (Color::Yellow, Color::DarkGray),
            SpawnField::Prompt => (Color::DarkGray, Color::Yellow),
        };
        self.project.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(proj_color))
                .title(" Project "),
        );
        self.prompt.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(prompt_color))
                .title(" Prompt "),
        );
    }
}

impl Default for SpawnModal {
    fn default() -> Self {
        Self::new()
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
    /// TextArea for text input modals (deny-with-message, approve-with-context, edit-input).
    pub textarea: Option<TextArea<'static>>,
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
            textarea: None,
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
            textarea: None,
        }
    }

    pub fn spawn_agent() -> Self {
        Self {
            title: "Spawn Agent".into(),
            body: "Tab to switch fields, Enter to spawn, Esc to cancel".into(),
            action: ModalAction::SpawnAgent,
            target_id: None,
            spawn: Some(SpawnModal::new()),
            textarea: None,
        }
    }

    pub fn deny_with_message(id: Uuid) -> Self {
        Self {
            title: "Deny with Message".into(),
            body: "Type a reason (Claude will see this as feedback):".into(),
            action: ModalAction::DenyWithMessage,
            target_id: Some(id),
            spawn: None,
            textarea: Some(make_textarea("", "Enter feedback...")),
        }
    }

    pub fn approve_with_context(id: Uuid) -> Self {
        Self {
            title: "Approve with Context".into(),
            body: "Type additional context (injected into Claude's conversation):".into(),
            action: ModalAction::ApproveWithContext,
            target_id: Some(id),
            spawn: None,
            textarea: Some(make_textarea("", "Enter context...")),
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
            textarea: Some(make_textarea(&initial, "")),
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
            textarea: None,
        }
    }

    pub fn confirm_ask_defer(id: Uuid) -> Self {
        Self {
            title: "Defer".into(),
            body: "Pass to Claude's native permission prompt?\n\n  Y = defer  |  N / Esc = cancel".into(),
            action: ModalAction::AskDefer,
            target_id: Some(id),
            spawn: None,
            textarea: None,
        }
    }
}
