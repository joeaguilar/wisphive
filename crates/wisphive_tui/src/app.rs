use std::path::PathBuf;

use uuid::Uuid;
use wisphive_protocol::{AgentInfo, DecisionRequest};

use crate::modal::Modal;

/// Which panel currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPanel {
    Queue,
    Agents,
    Projects,
}

impl FocusPanel {
    pub fn next(self) -> Self {
        match self {
            Self::Queue => Self::Agents,
            Self::Agents => Self::Projects,
            Self::Projects => Self::Queue,
        }
    }
}

/// Application state for the TUI.
pub struct App {
    /// Pending decisions in the queue.
    pub queue: Vec<DecisionRequest>,
    /// Currently selected index in the queue.
    pub queue_index: usize,
    /// Connected agents.
    pub agents: Vec<AgentInfo>,
    /// Known projects (derived from agent connections).
    pub projects: Vec<ProjectStatus>,
    /// Which panel has focus.
    pub focus: FocusPanel,
    /// Active modal dialog (if any).
    pub modal: Option<Modal>,
    /// Filter string for the queue (set by '/' search).
    pub filter: Option<String>,
    /// Whether the app should exit.
    pub should_quit: bool,
    /// Whether we're connected to the daemon.
    pub connected: bool,
    /// Whether the user is in filter input mode.
    pub filter_input_mode: bool,
    /// Buffer for filter input.
    pub filter_buffer: String,
}

/// Aggregated project status for the dashboard.
pub struct ProjectStatus {
    pub path: PathBuf,
    pub agent_count: usize,
    pub pending_count: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            queue: Vec::new(),
            queue_index: 0,
            agents: Vec::new(),
            projects: Vec::new(),
            focus: FocusPanel::Queue,
            modal: None,
            filter: None,
            should_quit: false,
            connected: false,
            filter_input_mode: false,
            filter_buffer: String::new(),
        }
    }

    /// Get the currently selected decision request, if any.
    pub fn selected_request(&self) -> Option<&DecisionRequest> {
        let filtered = self.filtered_queue();
        filtered.get(self.queue_index).copied()
    }

    /// Get the queue filtered by the current filter string.
    pub fn filtered_queue(&self) -> Vec<&DecisionRequest> {
        match &self.filter {
            None => self.queue.iter().collect(),
            Some(f) => {
                let f = f.to_lowercase();
                self.queue
                    .iter()
                    .filter(|req| {
                        req.tool_name.to_lowercase().contains(&f)
                            || req.agent_id.to_lowercase().contains(&f)
                            || req.project.to_string_lossy().to_lowercase().contains(&f)
                    })
                    .collect()
            }
        }
    }

    /// Move selection up in the queue.
    pub fn queue_up(&mut self) {
        if self.queue_index > 0 {
            self.queue_index -= 1;
        }
    }

    /// Move selection down in the queue.
    pub fn queue_down(&mut self) {
        let len = self.filtered_queue().len();
        if len > 0 && self.queue_index < len - 1 {
            self.queue_index += 1;
        }
    }

    /// Cycle focus to the next panel.
    pub fn cycle_focus(&mut self) {
        self.focus = self.focus.next();
    }

    /// Rebuild the projects list from current agents and queue.
    pub fn rebuild_projects(&mut self) {
        use std::collections::HashMap;

        let mut map: HashMap<PathBuf, (usize, usize)> = HashMap::new();

        for agent in &self.agents {
            let entry = map.entry(agent.project.clone()).or_default();
            entry.0 += 1;
        }

        for req in &self.queue {
            let entry = map.entry(req.project.clone()).or_default();
            entry.1 += 1;
        }

        self.projects = map
            .into_iter()
            .map(|(path, (agent_count, pending_count))| ProjectStatus {
                path,
                agent_count,
                pending_count,
            })
            .collect();

        self.projects.sort_by(|a, b| a.path.cmp(&b.path));
    }

    /// Remove a decision from the queue by ID.
    pub fn remove_decision(&mut self, id: Uuid) {
        self.queue.retain(|r| r.id != id);
        let len = self.filtered_queue().len();
        if self.queue_index >= len && len > 0 {
            self.queue_index = len - 1;
        }
        self.rebuild_projects();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
