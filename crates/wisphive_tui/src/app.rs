use std::path::PathBuf;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use uuid::Uuid;

use wisphive_protocol::{
    AgentInfo, AutoApproveLevel, DecisionRequest, HistoryEntry, TerminalSessionMeta, ToolRule,
};

use serde::Deserialize;

use crate::modal::Modal;

/// All known tools for the config toggle list — derived from the single source
/// of truth in `wisphive_protocol` (itr#121) rather than hardcoded here, so the
/// list can never drift from the tiers the hook actually enforces. Indexed by
/// [`ConfigRow::Tool`]; the derivation is deterministic so indices are stable.
pub static ALL_TOOLS: LazyLock<Vec<&'static str>> =
    LazyLock::new(wisphive_protocol::all_known_tools);

/// A row in the config view — either the level selector, a tool, an inline rule, or an event toggle.
#[derive(Debug, Clone)]
pub enum ConfigRow {
    Level,
    Tool(usize),
    Rule {
        tool_idx: usize,
        rule_idx: usize,
        is_deny: bool,
    },
    /// Toggle for event-type auto-approve (key name in config.json).
    EventToggle(&'static str),
}

/// Event types that can be auto-approved, with display names.
pub const EVENT_TOGGLES: &[(&str, &str)] = &[
    ("auto_approve_stop", "Stop/SubagentStop"),
    ("auto_approve_user_prompt", "UserPromptSubmit"),
    ("auto_approve_config_change", "ConfigChange"),
];

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
    #[serde(default)]
    pub auto_approve_stop: Option<bool>,
    #[serde(default)]
    pub auto_approve_user_prompt: Option<bool>,
    #[serde(default)]
    pub auto_approve_config_change: Option<bool>,
}

/// One config-view change handed to the CLI for off-runtime persistence.
/// Applying only the selected setting prevents a stale TUI view from replacing
/// disjoint changes made by the daemon, web UI, or another CLI process.
pub enum ConfigMutation {
    AutoApproveLevel(AutoApproveLevel),
    EventToggle {
        key: String,
        enabled: bool,
    },
    ToolOverride {
        tool: String,
        add: bool,
        remove: bool,
    },
    ToolRulePattern {
        tool: String,
        pattern: String,
        deny: bool,
        include: bool,
    },
}

impl ConfigMutation {
    pub fn apply_to(
        self,
        obj: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        match self {
            Self::AutoApproveLevel(level) => {
                obj.insert(
                    "auto_approve_level".into(),
                    serde_json::Value::String(level.to_string()),
                );
            }
            Self::EventToggle { key, enabled } => {
                obj.insert(key, serde_json::Value::Bool(enabled));
            }
            Self::ToolOverride { tool, add, remove } => {
                update_string_list(obj, "auto_approve_add", &tool, add)?;
                update_string_list(obj, "auto_approve_remove", &tool, remove)?;
            }
            Self::ToolRulePattern {
                tool,
                pattern,
                deny,
                include,
            } => {
                let mut rules = match obj.get("tool_rules") {
                    None => serde_json::Map::new(),
                    Some(serde_json::Value::Object(rules)) => rules.clone(),
                    Some(_) => return Err("tool_rules must be a JSON object".into()),
                };
                let mut rule = match rules.get(&tool) {
                    None => serde_json::Map::new(),
                    Some(serde_json::Value::Object(rule)) => rule.clone(),
                    Some(_) => return Err(format!("tool_rules.{tool} must be a JSON object")),
                };
                let key = if deny {
                    "deny_patterns"
                } else {
                    "allow_patterns"
                };
                update_string_list(&mut rule, key, &pattern, include)?;
                if rule.is_empty() {
                    rules.remove(&tool);
                } else {
                    rules.insert(tool, serde_json::Value::Object(rule));
                }
                if rules.is_empty() {
                    obj.remove("tool_rules");
                } else {
                    obj.insert("tool_rules".into(), serde_json::Value::Object(rules));
                }
            }
        }
        Ok(())
    }
}

fn update_string_list(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    item: &str,
    include: bool,
) -> Result<(), String> {
    let mut items = match obj.get(key) {
        None => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key} must contain only strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(format!("{key} must be a JSON array")),
    };

    items.retain(|existing| existing != item);
    if include {
        items.push(item.to_owned());
    }
    if items.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(
            key.into(),
            serde_json::Value::Array(items.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    Ok(())
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
    /// Project explorer.
    ProjectsExplorer,
    /// List of wisphive-managed PTY terminals.
    TerminalList,
    /// Attached to a running terminal — live PTY rendered via vt100.
    TerminalView,
    /// Replaying a terminal session's recorded events.
    TerminalReplay,
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
    /// Currently selected index in the agents panel.
    pub agents_index: usize,
    /// Known projects (derived from agent connections).
    pub projects: Vec<ProjectStatus>,
    /// Currently selected index in the projects panel.
    pub projects_panel_index: usize,
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
    /// Event-type auto-approve toggles (keyed by config.json field name).
    pub config_event_toggles: HashMap<String, bool>,
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
    /// Project summaries for the project explorer.
    pub project_summaries: Vec<wisphive_protocol::ProjectSummary>,
    /// Currently selected index in the project explorer.
    pub project_summaries_index: usize,
    /// Agent IDs that have been stopped (approved Stop events).
    pub stopped_agents: HashSet<String>,
    /// Whether the detail view is showing rendered markdown preview.
    pub markdown_preview: bool,

    // ── Terminal sessions ────────────────────────────────────────
    /// Known terminal sessions (running + historical).
    pub terminals: Vec<TerminalSessionMeta>,
    /// Currently selected index in the terminals list.
    pub terminals_index: usize,
    /// Live attached terminal state (vt100 parser + metadata).
    pub active_terminal: Option<ActiveTerminal>,
    /// Replay session state.
    pub replay_terminal: Option<ActiveTerminal>,
    /// Last time Esc was pressed while attached to a terminal.
    /// Used to detect double-tap detach (two Escs within a short window).
    pub last_terminal_esc: Option<std::time::Instant>,

    /// Active daemon resource alerts (audit archive size, low disk), at most one
    /// per kind. Rendered as a banner. Wisphive never deletes audit data; the
    /// daemon raises these instead (itr#340).
    pub disk_alerts: Vec<DiskAlertNotice>,
}

/// A single active resource alert surfaced by the daemon.
pub struct DiskAlertNotice {
    pub kind: wisphive_protocol::DiskAlertKind,
    pub message: String,
}

/// Client-side state for a live (or replayed) terminal session.
pub struct ActiveTerminal {
    pub id: Uuid,
    pub label: Option<String>,
    pub command: String,
    pub cols: u16,
    pub rows: u16,
    pub parser: vt100::Parser,
    pub ended: bool,
    /// Highest seq we've applied to the parser. Used to drop late/reordered frames.
    pub last_seq: u64,
}

impl ActiveTerminal {
    pub fn new(meta: &TerminalSessionMeta) -> Self {
        Self {
            id: meta.id,
            label: meta.label.clone(),
            command: meta.command.clone(),
            cols: meta.cols,
            rows: meta.rows,
            parser: vt100::Parser::new(meta.rows, meta.cols, 0),
            ended: matches!(
                meta.status,
                wisphive_protocol::TerminalStatus::Exited
                    | wisphive_protocol::TerminalStatus::Killed
                    | wisphive_protocol::TerminalStatus::Orphaned
            ),
            last_seq: 0,
        }
    }

    /// Feed a catchup snapshot (vt100 `contents_formatted()` output).
    pub fn feed_catchup(&mut self, screen: &[u8]) {
        // Reset parser state and replay the catchup buffer.
        self.parser = vt100::Parser::new(self.rows, self.cols, 0);
        self.parser.process(screen);
    }

    /// Feed a live chunk. Returns false if the chunk is out of order.
    pub fn feed_chunk(&mut self, seq: u64, bytes: &[u8]) -> bool {
        if seq < self.last_seq {
            return false;
        }
        self.last_seq = seq;
        self.parser.process(bytes);
        true
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.parser.set_size(rows, cols);
    }
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
            agents_index: 0,
            projects: Vec::new(),
            projects_panel_index: 0,
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
            config_event_toggles: HashMap::new(),
            sessions: Vec::new(),
            sessions_index: 0,
            session_timeline_agent_id: None,
            session_timeline: Vec::new(),
            session_timeline_index: 0,
            session_timeline_page: 0,
            session_timeline_has_more: false,
            project_summaries: Vec::new(),
            project_summaries_index: 0,
            stopped_agents: HashSet::new(),
            markdown_preview: false,
            terminals: Vec::new(),
            terminals_index: 0,
            active_terminal: None,
            replay_terminal: None,
            last_terminal_esc: None,
            disk_alerts: Vec::new(),
        }
    }

    /// Apply a daemon `disk_alert`: a raise (`active`) upserts the alert for its
    /// kind; a clear (`!active`) removes it. Keeps at most one per kind.
    pub fn apply_disk_alert(
        &mut self,
        kind: wisphive_protocol::DiskAlertKind,
        active: bool,
        message: String,
    ) {
        self.disk_alerts.retain(|a| a.kind != kind);
        if active {
            self.disk_alerts.push(DiskAlertNotice { kind, message });
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

    /// Move selection up in the agents panel.
    pub fn agents_up(&mut self) {
        if self.agents_index > 0 {
            self.agents_index -= 1;
        }
    }

    /// Move selection down in the agents panel.
    pub fn agents_down(&mut self) {
        let len = self.agents.len();
        if len > 0 && self.agents_index < len - 1 {
            self.agents_index += 1;
        }
    }

    /// Get the currently selected agent.
    pub fn selected_agent(&self) -> Option<&AgentInfo> {
        self.agents.get(self.agents_index)
    }

    /// Move selection up in the projects panel.
    pub fn projects_panel_up(&mut self) {
        if self.projects_panel_index > 0 {
            self.projects_panel_index -= 1;
        }
    }

    /// Move selection down in the projects panel.
    pub fn projects_panel_down(&mut self) {
        let len = self.projects.len();
        if len > 0 && self.projects_panel_index < len - 1 {
            self.projects_panel_index += 1;
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

    /// Get the hook event type of the current detail request.
    pub fn detail_event_type(&self) -> wisphive_protocol::HookEventType {
        self.detail_request()
            .map(|r| r.hook_event_name)
            .unwrap_or_default()
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
        // Load event toggles with defaults (Stop=false, UserPrompt=true, ConfigChange=true)
        self.config_event_toggles.clear();
        self.config_event_toggles.insert(
            "auto_approve_stop".into(),
            config.auto_approve_stop.unwrap_or(false),
        );
        self.config_event_toggles.insert(
            "auto_approve_user_prompt".into(),
            config.auto_approve_user_prompt.unwrap_or(true),
        );
        self.config_event_toggles.insert(
            "auto_approve_config_change".into(),
            config.auto_approve_config_change.unwrap_or(true),
        );
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
        // Event-type auto-approve toggles
        for (key, _) in EVENT_TOGGLES {
            rows.push(ConfigRow::EventToggle(key));
        }
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
        if self
            .session_timeline
            .get(self.session_timeline_index)
            .is_some()
        {
            self.history = self.session_timeline.clone();
            self.history_index = self.session_timeline_index;
            self.detail_scroll = 0;
            self.push_view(ViewMode::HistoryDetail);
        }
    }

    // ── Project explorer helpers ──

    pub fn enter_projects_view(&mut self) {
        self.project_summaries_index = 0;
        self.push_view(ViewMode::ProjectsExplorer);
    }

    pub fn exit_projects_view(&mut self) {
        self.project_summaries.clear();
        self.project_summaries_index = 0;
        self.navigate_back();
    }

    pub fn projects_up(&mut self) {
        if self.project_summaries_index > 0 {
            self.project_summaries_index -= 1;
        }
    }

    pub fn projects_down(&mut self) {
        let len = self.project_summaries.len();
        if len > 0 && self.project_summaries_index < len - 1 {
            self.project_summaries_index += 1;
        }
    }

    pub fn selected_project_summary(&self) -> Option<&wisphive_protocol::ProjectSummary> {
        self.project_summaries.get(self.project_summaries_index)
    }

    // ── Terminal session view helpers ──

    pub fn enter_terminal_list_view(&mut self) {
        self.terminals_index = 0;
        self.push_view(ViewMode::TerminalList);
    }

    pub fn exit_terminal_list_view(&mut self) {
        self.navigate_back();
    }

    pub fn terminals_up(&mut self) {
        if self.terminals_index > 0 {
            self.terminals_index -= 1;
        }
    }

    pub fn terminals_down(&mut self) {
        let len = self.terminals.len();
        if len > 0 && self.terminals_index < len - 1 {
            self.terminals_index += 1;
        }
    }

    pub fn selected_terminal(&self) -> Option<&TerminalSessionMeta> {
        self.terminals.get(self.terminals_index)
    }

    pub fn enter_terminal_view(&mut self, meta: &TerminalSessionMeta) {
        self.active_terminal = Some(ActiveTerminal::new(meta));
        self.push_view(ViewMode::TerminalView);
    }

    pub fn exit_terminal_view(&mut self) {
        self.active_terminal = None;
        self.navigate_back();
    }

    pub fn enter_terminal_replay_view(&mut self, meta: &TerminalSessionMeta) {
        self.replay_terminal = Some(ActiveTerminal::new(meta));
        self.push_view(ViewMode::TerminalReplay);
    }

    pub fn exit_terminal_replay_view(&mut self) {
        self.replay_terminal = None;
        self.navigate_back();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_mutations_preserve_disjoint_and_unrelated_raw_fields() {
        let mut obj = serde_json::json!({
            "auto_approve_add": ["Bash", "Read"],
            "auto_approve_remove": ["Write"],
            "tool_rules": {
                "Bash": {"deny_patterns": ["rm -rf"]},
                "Read": {"future_rule_field": true}
            },
            "future_tui_field": {"keep": true},
            "retention_max_rows": 1234,
        })
        .as_object()
        .unwrap()
        .clone();

        ConfigMutation::AutoApproveLevel(AutoApproveLevel::Execute)
            .apply_to(&mut obj)
            .unwrap();
        ConfigMutation::EventToggle {
            key: "auto_approve_stop".into(),
            enabled: true,
        }
        .apply_to(&mut obj)
        .unwrap();
        ConfigMutation::ToolOverride {
            tool: "Edit".into(),
            add: true,
            remove: false,
        }
        .apply_to(&mut obj)
        .unwrap();
        ConfigMutation::ToolRulePattern {
            tool: "Bash".into(),
            pattern: "sudo".into(),
            deny: true,
            include: true,
        }
        .apply_to(&mut obj)
        .unwrap();

        assert_eq!(obj["auto_approve_level"], "execute");
        assert_eq!(
            obj["auto_approve_add"],
            serde_json::json!(["Bash", "Read", "Edit"])
        );
        assert_eq!(obj["auto_approve_remove"], serde_json::json!(["Write"]));
        assert_eq!(obj["auto_approve_stop"], true);
        assert_eq!(
            obj["tool_rules"]["Bash"]["deny_patterns"],
            serde_json::json!(["rm -rf", "sudo"])
        );
        assert_eq!(obj["tool_rules"]["Read"]["future_rule_field"], true);
        assert_eq!(obj["future_tui_field"]["keep"], true);
        assert_eq!(obj["retention_max_rows"], 1234);
    }

    #[test]
    fn config_mutation_rejects_malformed_managed_fields() {
        let mut obj = serde_json::json!({"auto_approve_add": "Bash"})
            .as_object()
            .unwrap()
            .clone();

        let error = ConfigMutation::ToolOverride {
            tool: "Edit".into(),
            add: true,
            remove: false,
        }
        .apply_to(&mut obj)
        .unwrap_err();

        assert!(error.contains("auto_approve_add must be a JSON array"));
    }
}
