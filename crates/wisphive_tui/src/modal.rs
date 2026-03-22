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
    /// Answer an AskUserQuestion with typed text.
    AnswerQuestion,
    /// Pick a project from the list, then transition to SpawnAgent.
    PickProject,
}

/// Active input field in the spawn agent modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnField {
    Project,
    Prompt,
    Model,
    Reasoning,
    MaxTurns,
}

impl SpawnField {
    pub fn next(self) -> Self {
        match self {
            Self::Project => Self::Prompt,
            Self::Prompt => Self::Model,
            Self::Model => Self::Reasoning,
            Self::Reasoning => Self::MaxTurns,
            Self::MaxTurns => Self::Project,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Project => Self::MaxTurns,
            Self::Prompt => Self::Project,
            Self::Model => Self::Prompt,
            Self::Reasoning => Self::Model,
            Self::MaxTurns => Self::Reasoning,
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
    pub model: TextArea<'static>,
    pub reasoning: TextArea<'static>,
    pub max_turns: TextArea<'static>,
    pub active_field: SpawnField,
}

impl SpawnModal {
    pub fn new() -> Self {
        let project_path = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut s = Self {
            project: make_textarea(&project_path, "/path/to/project"),
            prompt: make_textarea("", "Enter prompt for the agent..."),
            model: make_textarea("", "sonnet, opus (optional)"),
            reasoning: make_textarea("", "low/med/high (optional)"),
            max_turns: make_textarea("", "e.g. 10 (optional)"),
            active_field: SpawnField::Prompt,
        };
        s.apply_blocks();
        s
    }

    pub fn project_path(&self) -> PathBuf {
        PathBuf::from(&self.project.lines()[0])
    }

    /// Replace the project field text (used when spawning from project view).
    pub fn set_project(&mut self, path: &str) {
        self.project = make_textarea(path, "/path/to/project");
        self.apply_blocks();
    }

    pub fn model_value(&self) -> Option<String> {
        let val = self.model.lines()[0].trim().to_string();
        if val.is_empty() { None } else { Some(val) }
    }

    pub fn reasoning_value(&self) -> Option<String> {
        let val = self.reasoning.lines()[0].trim().to_string();
        if val.is_empty() { None } else { Some(val) }
    }

    pub fn max_turns_value(&self) -> Option<u32> {
        self.max_turns.lines()[0].trim().parse().ok()
    }

    pub fn active_textarea(&mut self) -> &mut TextArea<'static> {
        match self.active_field {
            SpawnField::Project => &mut self.project,
            SpawnField::Prompt => &mut self.prompt,
            SpawnField::Model => &mut self.model,
            SpawnField::Reasoning => &mut self.reasoning,
            SpawnField::MaxTurns => &mut self.max_turns,
        }
    }

    /// Update border styles to reflect which field is active.
    pub fn update_focus_styles(&mut self) {
        self.apply_blocks();
    }

    fn apply_blocks(&mut self) {
        let active = self.active_field;
        let color = |f: SpawnField| if f == active { Color::Yellow } else { Color::DarkGray };
        self.project.set_block(Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(color(SpawnField::Project))).title(" Project "));
        self.prompt.set_block(Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(color(SpawnField::Prompt))).title(" Prompt "));
        self.model.set_block(Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(color(SpawnField::Model))).title(" Model "));
        self.reasoning.set_block(Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(color(SpawnField::Reasoning))).title(" Reasoning "));
        self.max_turns.set_block(Block::default().borders(Borders::ALL)
            .border_style(Style::default().fg(color(SpawnField::MaxTurns))).title(" Max Turns "));
    }
}

impl Default for SpawnModal {
    fn default() -> Self {
        Self::new()
    }
}

/// State for the project picker modal.
pub struct PickerState {
    pub index: usize,
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
    /// State for the project picker modal.
    pub picker: Option<PickerState>,
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
            picker: None,
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
            picker: None,
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
            picker: None,
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
            picker: None,
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
            picker: None,
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
            picker: None,
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
            picker: None,
        }
    }

    pub fn answer_question(id: Uuid) -> Self {
        Self {
            title: "Answer Question".into(),
            body: "Type your response (sent as the answer to Claude):".into(),
            action: ModalAction::AnswerQuestion,
            target_id: Some(id),
            spawn: None,
            textarea: Some(make_textarea("", "Type your answer...")),
            picker: None,
        }
    }

    pub fn pick_project() -> Self {
        Self {
            title: "Pick Project".into(),
            body: "Select a project to spawn an agent (j/k navigate, Enter select, Esc cancel):".into(),
            action: ModalAction::PickProject,
            target_id: None,
            spawn: None,
            textarea: None,
            picker: Some(PickerState { index: 0 }),
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
            picker: None,
        }
    }
}
