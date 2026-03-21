use std::path::PathBuf;

use uuid::Uuid;
use std::collections::HashMap;

use wisphive_protocol::{AgentInfo, AutoApproveLevel, DecisionRequest, HistoryEntry, ToolRule};

use serde::Deserialize;

use crate::modal::Modal;

/// All known tools for the config toggle list.
pub const ALL_TOOLS: &[&str] = &[
    // Read tier
    "Read", "Glob", "Grep", "LS", "LSP", "NotebookRead",
    "WebSearch", "WebFetch",
    "Agent", "Skill", "ToolSearch", "AskUserQuestion",
    "EnterPlanMode", "ExitPlanMode", "EnterWorktree", "ExitWorktree",
    "TaskCreate", "TaskUpdate", "TaskGet", "TaskList", "TaskOutput", "TaskStop", "TodoRead",
    "CronList",
    // Write tier
    "Edit", "Write", "NotebookEdit", "CronCreate", "CronDelete",
    // Execute tier
    "Bash",
];

/// A row in the config view — either the level selector, a tool, or an inline rule.
#[derive(Debug, Clone)]
pub enum ConfigRow {
    Level,
    Tool(usize),
    Rule { tool_idx: usize, rule_idx: usize, is_deny: bool },
}

/// Minimal config snapshot for reading auto-approve settings from config.json.
#[derive(Deserialize, Default)]
pub struct ConfigSnapshot {
    #[serde(default)]
    pub auto_approve_level: Option<AutoApproveLevel>,
    #[serde(default)]
    pub auto_approve_add: Option<Vec<String>>,
    #[serde(default)]
    pub auto_approve_remove: Option<Vec<String>>,
    #[serde(default)]
    pub tool_rules: Option<HashMap<String, ToolRule>>,
}

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
    /// Configuration panel.
    Config,
    /// Session list browser.
    Sessions,
    /// Timeline for a single session.
    SessionTimeline,
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
    /// View navigation history (back stack).
    pub view_back_stack: Vec<ViewMode>,
    /// View navigation forward stack.
    pub view_forward_stack: Vec<ViewMode>,
    /// Current history page (0-indexed).
    pub history_page: usize,
    /// Whether there are more history pages available.
    pub history_has_more: bool,
    /// Current auto-approve level in config view.
    pub config_level: AutoApproveLevel,
    /// Current config selection index (0=level, 1+=tools for add/remove).
    pub config_index: usize,
    /// Tools added as overrides.
    pub config_add: Vec<String>,
    /// Tools removed as overrides.
    pub config_remove: Vec<String>,
    /// Content-aware rules per tool.
    pub config_tool_rules: HashMap<String, ToolRule>,
    /// Whether the config view is in rule-input mode (typing a new pattern).
    pub config_rule_input_mode: bool,
    /// Buffer for rule pattern input.
    pub config_rule_buffer: String,
    /// Tool name the rule is being added to.
    pub config_rule_target_tool: Option<String>,
    /// Whether the new rule is a deny pattern (true) or allow pattern (false).
    pub config_rule_is_deny: bool,
    /// Session summaries (live + historical).
    pub sessions: Vec<wisphive_protocol::SessionSummary>,
    /// Currently selected index in the sessions list.
    pub sessions_index: usize,
    /// Agent ID of the session timeline being viewed.
    pub session_timeline_agent_id: Option<String>,
    /// Timeline entries for the current session.
    pub session_timeline: Vec<HistoryEntry>,
    /// Currently selected index in the session timeline.
    pub session_timeline_index: usize,
    /// Current page of the session timeline.
    pub session_timeline_page: usize,
    /// Whether there are more timeline pages.
    pub session_timeline_has_more: bool,
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
            view_back_stack: Vec::new(),
            view_forward_stack: Vec::new(),
            history_page: 0,
            history_has_more: false,
            config_level: AutoApproveLevel::default(),
            config_index: 0,
            config_add: Vec::new(),
            config_remove: Vec::new(),
            config_tool_rules: HashMap::new(),
            config_rule_input_mode: false,
            config_rule_buffer: String::new(),
            config_rule_target_tool: None,
            config_rule_is_deny: true,
            sessions: Vec::new(),
            sessions_index: 0,
            session_timeline_agent_id: None,
            session_timeline: Vec::new(),
            session_timeline_index: 0,
            session_timeline_page: 0,
            session_timeline_has_more: false,
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

    /// Push current view onto back stack and switch to a new view.
    fn push_view(&mut self, new_view: ViewMode) {
        self.view_back_stack.push(self.view_mode);
        self.view_forward_stack.clear();
        self.view_mode = new_view;
    }

    /// Navigate back to the previous view. Returns true if there was a view to go back to.
    pub fn navigate_back(&mut self) -> bool {
        if let Some(prev) = self.view_back_stack.pop() {
            self.view_forward_stack.push(self.view_mode);
            self.view_mode = prev;
            self.detail_scroll = 0;
            true
        } else {
            false
        }
    }

    /// Navigate forward to the next view. Returns true if there was a view to go forward to.
    pub fn navigate_forward(&mut self) -> bool {
        if let Some(next) = self.view_forward_stack.pop() {
            self.view_back_stack.push(self.view_mode);
            self.view_mode = next;
            self.detail_scroll = 0;
            true
        } else {
            false
        }
    }

    /// Enter the detail view for the currently selected queue item.
    pub fn enter_detail_view(&mut self) {
        if let Some(req) = self.selected_request() {
            self.detail_request_id = Some(req.id);
            self.detail_scroll = 0;
            self.push_view(ViewMode::Detail);
        }
    }

    /// Leave the detail view and return to the dashboard.
    pub fn exit_detail_view(&mut self) {
        self.detail_request_id = None;
        self.detail_scroll = 0;
        self.navigate_back();
    }

    /// Get the decision request currently being viewed in detail.
    pub fn detail_request(&self) -> Option<&DecisionRequest> {
        let id = self.detail_request_id?;
        self.queue.iter().find(|r| r.id == id)
    }

    /// Check if the current detail request is a PermissionRequest.
    pub fn detail_is_permission_request(&self) -> bool {
        self.detail_request()
            .and_then(|r| r.permission_suggestions.as_ref())
            .is_some()
    }

    /// Enter the history view.
    pub fn enter_history_view(&mut self, agent_id: Option<String>) {
        self.history_agent_filter = agent_id;
        self.history_index = 0;
        self.history_page = 0;
        self.history_has_more = false;
        self.push_view(ViewMode::History);
    }

    /// Leave the history view and return to the dashboard.
    pub fn exit_history_view(&mut self) {
        self.history.clear();
        self.history_index = 0;
        self.history_page = 0;
        self.history_has_more = false;
        self.history_agent_filter = None;
        self.history_search_query = None;
        self.history_search_mode = false;
        self.history_search_buffer.clear();
        self.navigate_back();
    }

    /// Enter the history detail view for the currently selected history entry.
    pub fn enter_history_detail_view(&mut self) {
        if self.history.get(self.history_index).is_some() {
            self.detail_scroll = 0;
            self.push_view(ViewMode::HistoryDetail);
        }
    }

    /// Leave the history detail view and return to the history list.
    pub fn exit_history_detail_view(&mut self) {
        self.detail_scroll = 0;
        self.navigate_back();
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

    /// Enter the config view, loading current settings from disk.
    pub fn enter_config_view(&mut self) {
        let config = Self::load_user_config();
        self.config_level = config.auto_approve_level.unwrap_or_default();
        self.config_add = config.auto_approve_add.unwrap_or_default();
        self.config_remove = config.auto_approve_remove.unwrap_or_default();
        self.config_tool_rules = config.tool_rules.unwrap_or_default();
        self.config_index = 0;
        self.config_rule_input_mode = false;
        self.config_rule_buffer.clear();
        self.config_rule_target_tool = None;
        self.push_view(ViewMode::Config);
    }

    /// Leave the config view.
    pub fn exit_config_view(&mut self) {
        self.navigate_back();
    }

    /// Save current config state to disk.
    pub fn save_config(&self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let path = std::path::PathBuf::from(home)
            .join(".wisphive")
            .join("config.json");

        let mut config = match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<serde_json::Value>(&content)
                .unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        };

        let obj = config.as_object_mut().unwrap();
        obj.insert(
            "auto_approve_level".into(),
            serde_json::Value::String(self.config_level.to_string()),
        );
        if self.config_add.is_empty() {
            obj.remove("auto_approve_add");
        } else {
            obj.insert(
                "auto_approve_add".into(),
                serde_json::Value::Array(
                    self.config_add.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                ),
            );
        }
        if self.config_remove.is_empty() {
            obj.remove("auto_approve_remove");
        } else {
            obj.insert(
                "auto_approve_remove".into(),
                serde_json::Value::Array(
                    self.config_remove
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }

        // Persist tool_rules — remove tools with empty rules
        let non_empty: HashMap<String, ToolRule> = self
            .config_tool_rules
            .iter()
            .filter(|(_, r)| !r.deny_patterns.is_empty() || !r.allow_patterns.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if non_empty.is_empty() {
            obj.remove("tool_rules");
        } else {
            obj.insert(
                "tool_rules".into(),
                serde_json::to_value(&non_empty).unwrap_or(serde_json::json!({})),
            );
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap_or_default());
    }

    fn load_user_config() -> ConfigSnapshot {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let path = std::path::PathBuf::from(home)
            .join(".wisphive")
            .join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => ConfigSnapshot::default(),
        }
    }

    /// Build a flat list of config rows for indexing in the config view.
    pub fn config_rows(&self) -> Vec<ConfigRow> {
        let mut rows = vec![ConfigRow::Level];
        for (i, tool) in ALL_TOOLS.iter().enumerate() {
            rows.push(ConfigRow::Tool(i));
            if let Some(rule) = self.config_tool_rules.get(*tool) {
                for (ri, _) in rule.deny_patterns.iter().enumerate() {
                    rows.push(ConfigRow::Rule {
                        tool_idx: i,
                        rule_idx: ri,
                        is_deny: true,
                    });
                }
                for (ri, _) in rule.allow_patterns.iter().enumerate() {
                    rows.push(ConfigRow::Rule {
                        tool_idx: i,
                        rule_idx: ri,
                        is_deny: false,
                    });
                }
            }
        }
        rows
    }

    // ── Session view helpers ──

    pub fn enter_sessions_view(&mut self) {
        self.sessions_index = 0;
        self.push_view(ViewMode::Sessions);
    }

    pub fn exit_sessions_view(&mut self) {
        self.sessions.clear();
        self.sessions_index = 0;
        self.navigate_back();
    }

    pub fn enter_session_timeline_view(&mut self, agent_id: String) {
        self.session_timeline_agent_id = Some(agent_id);
        self.session_timeline_index = 0;
        self.session_timeline_page = 0;
        self.session_timeline_has_more = false;
        self.push_view(ViewMode::SessionTimeline);
    }

    pub fn exit_session_timeline_view(&mut self) {
        self.session_timeline.clear();
        self.session_timeline_index = 0;
        self.session_timeline_page = 0;
        self.session_timeline_has_more = false;
        self.session_timeline_agent_id = None;
        self.navigate_back();
    }

    pub fn sessions_up(&mut self) {
        if self.sessions_index > 0 {
            self.sessions_index -= 1;
        }
    }

    pub fn sessions_down(&mut self) {
        let len = self.sessions.len();
        if len > 0 && self.sessions_index < len - 1 {
            self.sessions_index += 1;
        }
    }

    pub fn selected_session(&self) -> Option<&wisphive_protocol::SessionSummary> {
        self.sessions.get(self.sessions_index)
    }

    pub fn session_timeline_up(&mut self) {
        if self.session_timeline_index > 0 {
            self.session_timeline_index -= 1;
        }
    }

    pub fn session_timeline_down(&mut self) {
        let len = self.session_timeline.len();
        if len > 0 && self.session_timeline_index < len - 1 {
            self.session_timeline_index += 1;
        }
    }

    pub fn enter_timeline_detail_view(&mut self) {
        if self.session_timeline.get(self.session_timeline_index).is_some() {
            self.history = self.session_timeline.clone();
            self.history_index = self.session_timeline_index;
            self.detail_scroll = 0;
            self.push_view(ViewMode::HistoryDetail);
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
