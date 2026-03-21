use std::path::PathBuf;

use uuid::Uuid;
use wisphive_protocol::{AgentInfo, DecisionRequest, HistoryEntry};

use crate::modal::Modal;

/// Which screen the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Normal dashboard: queue, agents, projects panels.
    Dashboard,
    /// Full-screen detail view for a single decision request.
    Detail,
    /// History browser showing resolved decisions from the audit log.
    History,
    /// Full-screen detail view for a single history entry.
    HistoryDetail,
}

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
    /// Current view mode (dashboard, detail, or history).
    pub view_mode: ViewMode,
    /// Scroll offset for the detail view content area.
    pub detail_scroll: usize,
    /// The UUID of the decision being viewed in detail.
    pub detail_request_id: Option<Uuid>,
    /// Decision history from the audit log.
    pub history: Vec<HistoryEntry>,
    /// Currently selected index in the history list.
    pub history_index: usize,
    /// Agent ID filter for history view. None = all agents.
    pub history_agent_filter: Option<String>,
    /// Whether the user is in history search input mode.
    pub history_search_mode: bool,
    /// Buffer for history search input.
    pub history_search_buffer: String,
    /// Active search query (applied filter).
    pub history_search_query: Option<String>,
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
            view_mode: ViewMode::Dashboard,
            detail_scroll: 0,
            detail_request_id: None,
            history: Vec::new(),
            history_index: 0,
            history_agent_filter: None,
            history_search_mode: false,
            history_search_buffer: String::new(),
            history_search_query: None,
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

    /// Enter the detail view for the currently selected queue item.
    pub fn enter_detail_view(&mut self) {
        if let Some(req) = self.selected_request() {
            self.detail_request_id = Some(req.id);
            self.detail_scroll = 0;
            self.view_mode = ViewMode::Detail;
        }
    }

    /// Leave the detail view and return to the dashboard.
    pub fn exit_detail_view(&mut self) {
        self.view_mode = ViewMode::Dashboard;
        self.detail_request_id = None;
        self.detail_scroll = 0;
    }

    /// Get the decision request currently being viewed in detail.
    pub fn detail_request(&self) -> Option<&DecisionRequest> {
        let id = self.detail_request_id?;
        self.queue.iter().find(|r| r.id == id)
    }

    /// Enter the history view.
    pub fn enter_history_view(&mut self, agent_id: Option<String>) {
        self.history_agent_filter = agent_id;
        self.history_index = 0;
        self.view_mode = ViewMode::History;
    }

    /// Leave the history view and return to the dashboard.
    pub fn exit_history_view(&mut self) {
        self.view_mode = ViewMode::Dashboard;
        self.history.clear();
        self.history_index = 0;
        self.history_agent_filter = None;
        self.history_search_query = None;
        self.history_search_mode = false;
        self.history_search_buffer.clear();
    }

    /// Enter the history detail view for the currently selected history entry.
    pub fn enter_history_detail_view(&mut self) {
        if self.history.get(self.history_index).is_some() {
            self.detail_scroll = 0;
            self.view_mode = ViewMode::HistoryDetail;
        }
    }

    /// Leave the history detail view and return to the history list.
    pub fn exit_history_detail_view(&mut self) {
        self.view_mode = ViewMode::History;
        self.detail_scroll = 0;
    }

    /// Get the currently selected history entry.
    pub fn selected_history_entry(&self) -> Option<&HistoryEntry> {
        self.history.get(self.history_index)
    }

    /// Move selection up in the history list.
    pub fn history_up(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
        }
    }

    /// Move selection down in the history list.
    pub fn history_down(&mut self) {
        let len = self.history.len();
        if len > 0 && self.history_index < len - 1 {
            self.history_index += 1;
        }
    }

    /// Remove a decision from the queue by ID.
    pub fn remove_decision(&mut self, id: Uuid) {
        if self.detail_request_id == Some(id) {
            self.exit_detail_view();
        }
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
