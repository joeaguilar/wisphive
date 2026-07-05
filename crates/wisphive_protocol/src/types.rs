use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The type of agent connected to Wisphive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Codex,
    ClaudeCode,
    Red,
    LocalLlm,
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex => write!(f, "codex"),
            Self::ClaudeCode => write!(f, "claude_code"),
            Self::Red => write!(f, "red"),
            Self::LocalLlm => write!(f, "local_llm"),
        }
    }
}

fn default_agent_type() -> AgentType {
    AgentType::ClaudeCode
}

/// Metadata about a connected agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub connected_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// A permission rule from an agent PermissionRequest event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRule {
    #[serde(rename = "toolName")]
    pub tool_name: String,
    #[serde(rename = "ruleContent")]
    pub rule_content: String,
}

/// A permission suggestion from an agent PermissionRequest event.
///
/// Each suggestion represents one option the user can select
/// (e.g., "allow Bash(rm -rf node_modules) in local settings").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionSuggestion {
    /// "addRules" or "setMode"
    #[serde(rename = "type")]
    pub suggestion_type: String,
    /// Rules to add (for addRules type).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PermissionRule>,
    /// "allow" or "deny"
    pub behavior: String,
    /// "localSettings", "projectSettings", or "session"
    pub destination: String,
    /// For setMode: the mode to set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// The type of agent hook event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum HookEventType {
    #[default]
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    Elicitation,
    ElicitationResult,
    InstructionsLoaded,
    UserPromptSubmit,
    Stop,
    SubagentStop,
    SubagentStart,
    StopFailure,
    ConfigChange,
    TeammateIdle,
    TaskCompleted,
    WorktreeCreate,
    WorktreeRemove,
    PreCompact,
    PostCompact,
    SessionStart,
    SessionEnd,
    Notification,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for HookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreToolUse => write!(f, "PreToolUse"),
            Self::PostToolUse => write!(f, "PostToolUse"),
            Self::PostToolUseFailure => write!(f, "PostToolUseFailure"),
            Self::PermissionRequest => write!(f, "PermissionRequest"),
            Self::Elicitation => write!(f, "Elicitation"),
            Self::ElicitationResult => write!(f, "ElicitationResult"),
            Self::InstructionsLoaded => write!(f, "InstructionsLoaded"),
            Self::UserPromptSubmit => write!(f, "UserPromptSubmit"),
            Self::Stop => write!(f, "Stop"),
            Self::SubagentStop => write!(f, "SubagentStop"),
            Self::SubagentStart => write!(f, "SubagentStart"),
            Self::StopFailure => write!(f, "StopFailure"),
            Self::ConfigChange => write!(f, "ConfigChange"),
            Self::TeammateIdle => write!(f, "TeammateIdle"),
            Self::TaskCompleted => write!(f, "TaskCompleted"),
            Self::WorktreeCreate => write!(f, "WorktreeCreate"),
            Self::WorktreeRemove => write!(f, "WorktreeRemove"),
            Self::PreCompact => write!(f, "PreCompact"),
            Self::PostCompact => write!(f, "PostCompact"),
            Self::SessionStart => write!(f, "SessionStart"),
            Self::SessionEnd => write!(f, "SessionEnd"),
            Self::Notification => write!(f, "Notification"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

impl std::str::FromStr for HookEventType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "PreToolUse" => Ok(Self::PreToolUse),
            "PostToolUse" => Ok(Self::PostToolUse),
            "PostToolUseFailure" => Ok(Self::PostToolUseFailure),
            "PermissionRequest" => Ok(Self::PermissionRequest),
            "Elicitation" => Ok(Self::Elicitation),
            "ElicitationResult" => Ok(Self::ElicitationResult),
            "InstructionsLoaded" => Ok(Self::InstructionsLoaded),
            "UserPromptSubmit" => Ok(Self::UserPromptSubmit),
            "Stop" => Ok(Self::Stop),
            "SubagentStop" => Ok(Self::SubagentStop),
            "SubagentStart" => Ok(Self::SubagentStart),
            "StopFailure" => Ok(Self::StopFailure),
            "ConfigChange" => Ok(Self::ConfigChange),
            "TeammateIdle" => Ok(Self::TeammateIdle),
            "TaskCompleted" => Ok(Self::TaskCompleted),
            "WorktreeCreate" => Ok(Self::WorktreeCreate),
            "WorktreeRemove" => Ok(Self::WorktreeRemove),
            "PreCompact" => Ok(Self::PreCompact),
            "PostCompact" => Ok(Self::PostCompact),
            "SessionStart" => Ok(Self::SessionStart),
            "SessionEnd" => Ok(Self::SessionEnd),
            "Notification" => Ok(Self::Notification),
            _ => Ok(Self::Unknown),
        }
    }
}

/// A decision the human needs to make about a tool call or hook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub id: Uuid,
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    /// The agent hook event type. Defaults to PreToolUse for backward compat.
    #[serde(default)]
    pub hook_event_name: HookEventType,
    /// Agent-provided unique tool call ID for pre/post correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Permission suggestions from an agent PermissionRequest event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_suggestions: Option<Vec<PermissionSuggestion>>,
    /// Event-specific data (e.g., Elicitation schema, Stop reason, prompt text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_data: Option<serde_json::Value>,
    /// ID of the wisphive terminal session this call originated from, if any.
    /// Set via the `WISPHIVE_TERMINAL_SESSION_ID` env var propagated from a
    /// daemon-managed PTY down through the shell to the hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_session_id: Option<Uuid>,
}

/// The human's decision on a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Deny,
    /// Pass through to the agent's native permission prompt.
    Ask,
}

/// A rich decision carrying optional feedback, input modifications, and rules.
///
/// Flows through the oneshot channel and wire protocol. The `Decision` enum
/// remains binary for persistence/display; `RichDecision` is the transient payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RichDecision {
    /// The core decision.
    pub decision: Decision,
    /// Feedback message for the agent (deny reason or approval guidance).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Modified tool input to use instead of the original.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    /// If true, add this tool to the auto-approve list for future calls.
    #[serde(default)]
    pub always_allow: bool,
    /// Additional context to inject into the agent's conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// Selected permission suggestion (PermissionRequest only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_permission: Option<PermissionSuggestion>,
    /// Identity of the resolving client for the audit trail (itr#88):
    /// "human:tui" for the local TUI, "human:web:<device-id>" for an
    /// authenticated web device. None for internal/fallback resolutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
}

impl RichDecision {
    pub fn approve() -> Self {
        Self {
            decision: Decision::Approve,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
            selected_permission: None,
            resolver: None,
        }
    }

    pub fn deny() -> Self {
        Self {
            decision: Decision::Deny,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
            selected_permission: None,
            resolver: None,
        }
    }
}

impl From<Decision> for RichDecision {
    fn from(d: Decision) -> Self {
        Self {
            decision: d,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
            selected_permission: None,
            resolver: None,
        }
    }
}

/// Aggregated stats for one agent session (identified by agent_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub total_calls: u32,
    pub approved: u32,
    pub denied: u32,
    /// Whether this session is currently connected (live agent).
    pub is_live: bool,
    /// Number of currently pending decisions for this session.
    pub pending_count: u32,
}

/// Aggregated stats for a project across all agent sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project: PathBuf,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub total_calls: u32,
    pub approved: u32,
    pub denied: u32,
    pub agent_count: u32,
    pub pending_count: u32,
    pub has_live_agents: bool,
}

/// Request to spawn a new AI agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    /// Agent implementation to spawn.
    #[serde(default = "default_agent_type")]
    pub agent_type: AgentType,
    /// Working directory for the agent.
    pub project: PathBuf,
    /// Prompt to pass to the agent.
    pub prompt: String,
    /// Model to use (e.g. "sonnet", "opus"). None = agent default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Human-readable session name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Reasoning effort level (e.g. "low", "medium", "high").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<String>,
    /// Maximum number of agentic turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_turns: Option<u32>,
    /// Permission mode (e.g. "default", "plan", "bypassPermissions").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    /// Custom system prompt (replaces default).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub system_prompt: Option<String>,
    /// Additional system prompt text (appended to default).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub append_system_prompt: Option<String>,
    /// Restrict to specific tools.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Block specific tools.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub disallowed_tools: Option<Vec<String>>,
    /// Continue the most recent session.
    #[serde(default)]
    pub continue_session: bool,
    /// Resume a specific session by ID.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resume: Option<String>,
    /// Output format (json, stream-json, text).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_format: Option<String>,
    /// Enable verbose output.
    #[serde(default)]
    pub verbose: bool,
}

/// Metadata about a daemon-managed agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedAgent {
    pub agent_id: String,
    #[serde(default = "default_agent_type")]
    pub agent_type: AgentType,
    pub pid: u32,
    pub project: PathBuf,
    pub model: Option<String>,
    pub name: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
}

/// A tool execution result reported by the PostToolUse hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub agent_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_result: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    /// Agent-provided unique tool call ID for pre/post correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// Search criteria for the audit history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistorySearch {
    /// Free-text search across tool_input, tool_result, and tool_name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Filter by tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Filter by agent ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Maximum results (default 200).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Opaque correlation ID echoed back in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Only entries resolved at/after this instant (audit queries, itr#397).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<DateTime<Utc>>,
    /// Filter by project path (exact match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Filter by the layer/rule that made the decision (substring match on
    /// `decided_by`, e.g. "level:", "human", "always_ask").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// A resolved decision from the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub decision: Decision,
    pub requested_at: DateTime<Utc>,
    pub resolved_at: DateTime<Utc>,
    /// Tool execution result (captured by PostToolUse hook). None for old/denied entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<serde_json::Value>,
    /// Agent-provided unique tool call ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Hook event type (PreToolUse, Stop, UserPromptSubmit, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_event_name: Option<String>,
    /// ID of the wisphive terminal session this call originated from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_session_id: Option<Uuid>,
    /// The layer/rule that resolved this decision (itr#397 audit trail):
    /// "human", "timeout:approve", a hook rule like "level:all" /
    /// "auto_approve_add" / "tool_rules:Bash:allow_pattern" /
    /// "always_ask:intrinsic" / "event_toggle:auto_approve_stop", etc.
    /// None on rows from before the audit trail landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// Truncated SHA-256 of ~/.wisphive/config.json at decision time, so a
    /// policy weakening is correlatable with the decisions it produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
}

/// A compact live/snapshot audit event for decisions Wisphive resolved without
/// direct in-console action: auto-approved hook decisions, always-deferred
/// native prompts, and hook-side denials.
// `Eq` is intentionally omitted: `tool_input` is a `serde_json::Value`, which is
// `PartialEq` but not `Eq`. Nothing keys an `AuditDecision` in a HashMap/HashSet,
// so `PartialEq` alone is sufficient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditDecision {
    pub kind: AuditDecisionKind,
    pub decided_by: Option<String>,
    pub project: PathBuf,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_session_id: Option<Uuid>,
    pub tool_name: String,
    pub ts: DateTime<Utc>,
    /// The Claude Code `tool_use_id` of the gated call, when the source row carried one.
    /// For a DEFERRED native prompt this is the stable key that a later
    /// [`ServerMessage::DeferredResolved`] correlates against so the inbox can clear the
    /// exact "waiting in your terminal" row once the prompt is answered (itr#440 / #462).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Set on a DEFERRED row that has since been ANSWERED in the native prompt: the daemon
    /// stamped its `tool_result` (via `attach_tool_result`) so a reconnect snapshot can mark
    /// the deferral resolved rather than re-showing it as waiting (itr#461). None = still
    /// waiting (or not a deferral).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<bool>,
    /// Redacted tool input for DEFERRED native prompts (AskUserQuestion / ExitPlanMode /
    /// Elicitation), so the inbox can show the literal question + options. Present only
    /// for kind == Deferred; None for auto-approved/denied to keep the wire lean. Already
    /// secret-redacted upstream (hook redact::redact_value + itr#89).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
}

/// Category for [`AuditDecision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecisionKind {
    AutoApproved,
    Deferred,
    Denied,
}

// ── Terminal sessions ───────────────────────────────────────────────

/// Lifecycle status of a daemon-managed terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    /// The PTY is open and the child process is running.
    Running,
    /// The child process exited cleanly.
    Exited,
    /// The child was killed (by daemon shutdown or explicit close).
    Killed,
    /// The daemon restarted while the session was running; the PTY is gone.
    /// Event history can still be replayed from SQLite.
    Orphaned,
}

impl std::fmt::Display for TerminalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Exited => write!(f, "exited"),
            Self::Killed => write!(f, "killed"),
            Self::Orphaned => write!(f, "orphaned"),
        }
    }
}

impl std::str::FromStr for TerminalStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(Self::Running),
            "exited" => Ok(Self::Exited),
            "killed" => Ok(Self::Killed),
            "orphaned" => Ok(Self::Orphaned),
            _ => Err(format!("unknown terminal status: {s}")),
        }
    }
}

/// Direction of a terminal event in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDirection {
    /// Bytes the user typed (written to the PTY master).
    Input,
    /// Bytes emitted by the child process (read from the PTY master).
    Output,
    /// A PTY resize event. Payload encodes `cols,rows` as UTF-8 text.
    Resize,
}

impl std::fmt::Display for TerminalDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => write!(f, "input"),
            Self::Output => write!(f, "output"),
            Self::Resize => write!(f, "resize"),
        }
    }
}

impl std::str::FromStr for TerminalDirection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "input" => Ok(Self::Input),
            "output" => Ok(Self::Output),
            "resize" => Ok(Self::Resize),
            _ => Err(format!("unknown terminal direction: {s}")),
        }
    }
}

/// Metadata describing a terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionMeta {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The command that was spawned (argv[0]).
    pub command: String,
    /// Arguments passed to the command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Working directory the child was started in.
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub status: TerminalStatus,
    /// Optional user-assigned group label for sidebar organization. None = ungrouped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    /// Manual sort order within a group. Lower values render first. Defaults
    /// to `-started_at_ms` so newest-first is the natural order before the
    /// user drags anything.
    #[serde(default)]
    pub sort_order: i64,
}

/// Content-aware rule for a specific tool.
///
/// When a tool would be auto-approved, `deny_patterns` can block specific inputs
/// (sending them to the TUI). When a tool is NOT auto-approved, `allow_patterns`
/// can auto-approve specific inputs. Patterns are case-insensitive substrings
/// matched against the tool input (the `command` field for Bash, or the full
/// JSON-serialized input for other tools).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolRule {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_patterns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_patterns: Vec<String>,
}

/// Auto-approve permission levels. Higher levels include all tools from lower levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AutoApproveLevel {
    /// Nothing auto-approved. Every tool goes through the TUI.
    Off,
    /// Read-only + orchestration tools (default).
    #[default]
    Read,
    /// Level 1 + file modifications (Edit, Write, NotebookEdit).
    Write,
    /// Level 2 + shell execution (Bash).
    Execute,
    /// Everything auto-approved. TUI is monitoring-only.
    All,
}

impl AutoApproveLevel {
    /// Tools added at this specific tier (not cumulative).
    ///
    /// This is the single source of truth for the auto-approve tool lists
    /// across the whole workspace (itr#121). The hook's default fallback, the
    /// TUI/CLI toggle lists, and the web UI (via [`ToolTiers`] / `/api/tool-tiers`)
    /// all derive from here — removing a tool from a tier removes it everywhere.
    pub fn tier_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::Read => &[
                // File/content reading
                "Read",
                "Glob",
                "Grep",
                "LS",
                "LSP",
                "NotebookRead",
                // Web (read-only)
                "WebSearch",
                "WebFetch",
                // Orchestration
                // NOTE: AskUserQuestion / EnterPlanMode / ExitPlanMode are
                // deliberately NOT here. They carry a human answer back only
                // through the native prompt, so they are routed to an
                // always-defer classification in wisphive_hook instead of being
                // auto-approved at any level (see itr#380).
                "Agent",
                "Skill",
                "ToolSearch",
                "EnterWorktree",
                "ExitWorktree",
                // Task management
                "TaskCreate",
                "TaskUpdate",
                "TaskGet",
                "TaskList",
                "TaskOutput",
                "TaskStop",
                "TodoRead",
                // Scheduling
                "CronList",
            ],
            Self::Write => &["Edit", "Write", "NotebookEdit", "CronCreate", "CronDelete"],
            Self::Execute => &["Bash"],
            Self::All => &[], // All covers everything, checked separately
        }
    }

    /// Check if this level auto-approves the given tool.
    pub fn includes(&self, tool_name: &str) -> bool {
        if *self == Self::All {
            return true;
        }
        // Check all tiers at or below this level
        for level in &[Self::Off, Self::Read, Self::Write, Self::Execute] {
            if *level > *self {
                break;
            }
            if level.tier_tools().contains(&tool_name) {
                return true;
            }
        }
        false
    }
}

impl std::fmt::Display for AutoApproveLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
            Self::Execute => write!(f, "execute"),
            Self::All => write!(f, "all"),
        }
    }
}

/// Tools/events that ALWAYS defer to the agent's native prompt, regardless of
/// [`AutoApproveLevel`] and of the "dangerous" posture — `auto_approve_dangerous`
/// releases only operator-designated `always_ask` tools, never these. They
/// elicit a human answer that can only be carried back through the native
/// prompt (PermissionRequest / Elicitation), so auto-approving them would
/// silently resolve the prompt with no selection. The enforcement point is
/// `is_always_deferred` in wisphive_hook; operators extend the set via
/// `always_ask`, and `always_ask_remove` trims only those additions — the
/// intrinsic entries here cannot be removed. See itr#380, itr#388.
pub const DEFAULT_ALWAYS_ASK: &[&str] = &[
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "Elicitation",
];

/// Serializable view of the auto-approve tiers — derived entirely from
/// [`AutoApproveLevel::tier_tools`] and [`DEFAULT_ALWAYS_ASK`]. This is what the
/// web frontend reads from `GET /api/tool-tiers` instead of hardcoding the lists
/// in TypeScript, so the React bundle stays in lockstep with the Rust source
/// (itr#121). Serialize-only: the `&'static str` slices can't be deserialized.
#[derive(Debug, Clone, Serialize)]
pub struct ToolTiers {
    pub read: &'static [&'static str],
    pub write: &'static [&'static str],
    pub execute: &'static [&'static str],
    /// Always-defer set (questions / plan-mode / elicitations). Surfaced so the
    /// UI can display/label them, even though no level auto-approves them.
    pub always_ask: &'static [&'static str],
}

impl ToolTiers {
    /// The current tier definitions, straight from the single source of truth.
    pub fn current() -> Self {
        Self {
            read: AutoApproveLevel::Read.tier_tools(),
            write: AutoApproveLevel::Write.tier_tools(),
            execute: AutoApproveLevel::Execute.tier_tools(),
            always_ask: DEFAULT_ALWAYS_ASK,
        }
    }
}

/// Every tool the config UIs enumerate for their toggle lists: all auto-approve
/// tiers plus the always-defer set, in display order (read tier, then the
/// always-ask questions, then write, then execute). Single source for the TUI
/// `ALL_TOOLS` list and the CLI `auto-approve status` output (itr#121).
pub fn all_known_tools() -> Vec<&'static str> {
    let mut tools = Vec::new();
    tools.extend_from_slice(AutoApproveLevel::Read.tier_tools());
    tools.extend_from_slice(DEFAULT_ALWAYS_ASK);
    tools.extend_from_slice(AutoApproveLevel::Write.tier_tools());
    tools.extend_from_slice(AutoApproveLevel::Execute.tier_tools());
    tools
}

impl std::str::FromStr for AutoApproveLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" | "0" => Ok(Self::Off),
            "read" | "1" => Ok(Self::Read),
            "write" | "2" => Ok(Self::Write),
            "execute" | "exec" | "3" => Ok(Self::Execute),
            "all" | "4" => Ok(Self::All),
            _ => Err(format!(
                "unknown level: {s}. Valid: off, read, write, execute, all"
            )),
        }
    }
}

/// Filter criteria for batch operations on the decision queue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecisionFilter {
    /// Filter by tool name (e.g. "Bash", "Write").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Filter by project path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// Filter by agent type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
}

impl DecisionFilter {
    /// Returns true if the given request matches this filter.
    pub fn matches(&self, req: &DecisionRequest) -> bool {
        if let Some(ref tool) = self.tool_name
            && req.tool_name != *tool
        {
            return false;
        }
        if let Some(ref project) = self.project
            && req.project != *project
        {
            return false;
        }
        if let Some(ref agent_type) = self.agent_type
            && req.agent_type != *agent_type
        {
            return false;
        }
        true
    }
}

/// Wire-only summary of a project's Wisphive hook install state (itr#460).
///
/// A flattened mirror of the hook section of
/// `wisphive_daemon::project_audit::ProjectAudit`, kept in the protocol crate
/// so it can travel on a [`crate::ServerMessage`] without the protocol crate
/// depending on the daemon (that would be a dependency cycle). The daemon maps
/// its richer `ProjectAudit` down to this before sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHookStatus {
    /// The audited project directory.
    pub project: PathBuf,
    /// Global gating mode label ("active" / "off" / "missing" / "invalid: ...").
    pub mode: String,
    /// All expected Claude Code hook events are installed.
    pub claude_installed: bool,
    /// All expected Codex hook events are installed.
    pub codex_installed: bool,
    /// Union of Claude + Codex hook events that are still missing.
    pub missing_events: Vec<String>,
    /// Both agents have every expected hook installed.
    pub all_installed: bool,
    /// Both agents' hooks are installed and gating mode is active.
    pub all_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_request(
        tool: &str,
        agent_id: &str,
        project: &str,
        agent_type: AgentType,
    ) -> DecisionRequest {
        DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: agent_id.into(),
            agent_type,
            project: PathBuf::from(project),
            tool_name: tool.into(),
            tool_input: serde_json::Value::Null,
            timestamp: chrono::Utc::now(),
            hook_event_name: Default::default(),
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
        }
    }

    // ── AgentType ──────────────────────────────────────────────────────

    #[test]
    fn agent_type_serde_round_trip() {
        for (variant, expected_json) in [
            (AgentType::Codex, "\"codex\""),
            (AgentType::ClaudeCode, "\"claude_code\""),
            (AgentType::Red, "\"red\""),
            (AgentType::LocalLlm, "\"local_llm\""),
        ] {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(serialized, expected_json, "serialize {:?}", variant);
            let deserialized: AgentType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, variant, "deserialize {:?}", variant);
        }
    }

    #[test]
    fn agent_type_display() {
        assert_eq!(AgentType::Codex.to_string(), "codex");
        assert_eq!(AgentType::ClaudeCode.to_string(), "claude_code");
        assert_eq!(AgentType::Red.to_string(), "red");
        assert_eq!(AgentType::LocalLlm.to_string(), "local_llm");
    }

    // ── HookEventType ──────────────────────────────────────────────────

    #[test]
    fn hook_event_type_all_variants_round_trip() {
        let variants = [
            HookEventType::PreToolUse,
            HookEventType::PostToolUse,
            HookEventType::PostToolUseFailure,
            HookEventType::PermissionRequest,
            HookEventType::Elicitation,
            HookEventType::ElicitationResult,
            HookEventType::InstructionsLoaded,
            HookEventType::UserPromptSubmit,
            HookEventType::Stop,
            HookEventType::SubagentStop,
            HookEventType::SubagentStart,
            HookEventType::StopFailure,
            HookEventType::ConfigChange,
            HookEventType::TeammateIdle,
            HookEventType::TaskCompleted,
            HookEventType::WorktreeCreate,
            HookEventType::WorktreeRemove,
            HookEventType::PreCompact,
            HookEventType::PostCompact,
            HookEventType::SessionStart,
            HookEventType::SessionEnd,
            HookEventType::Notification,
        ];
        for variant in variants {
            let serialized = serde_json::to_string(&variant).unwrap();
            let deserialized: HookEventType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, variant, "round-trip failed for {:?}", variant);
        }
    }

    #[test]
    fn hook_event_type_unknown_serde() {
        let deserialized: HookEventType = serde_json::from_str("\"SomeFutureEvent\"").unwrap();
        assert_eq!(deserialized, HookEventType::Unknown);
    }

    #[test]
    fn hook_event_type_display_from_str_round_trip() {
        let variants = [
            HookEventType::PreToolUse,
            HookEventType::PostToolUse,
            HookEventType::PostToolUseFailure,
            HookEventType::PermissionRequest,
            HookEventType::Elicitation,
            HookEventType::ElicitationResult,
            HookEventType::InstructionsLoaded,
            HookEventType::UserPromptSubmit,
            HookEventType::Stop,
            HookEventType::SubagentStop,
            HookEventType::SubagentStart,
            HookEventType::StopFailure,
            HookEventType::ConfigChange,
            HookEventType::TeammateIdle,
            HookEventType::TaskCompleted,
            HookEventType::WorktreeCreate,
            HookEventType::WorktreeRemove,
            HookEventType::PreCompact,
            HookEventType::PostCompact,
            HookEventType::SessionStart,
            HookEventType::SessionEnd,
            HookEventType::Notification,
            HookEventType::Unknown,
        ];
        for variant in variants {
            let display = variant.to_string();
            let parsed: HookEventType = display.parse().unwrap();
            assert_eq!(
                parsed, variant,
                "Display→FromStr failed for {:?} (display={:?})",
                variant, display
            );
        }
    }

    #[test]
    fn hook_event_type_from_str_unknown() {
        let parsed: HookEventType = "TotallyMadeUp".parse().unwrap();
        assert_eq!(parsed, HookEventType::Unknown);
    }

    // ── Decision ───────────────────────────────────────────────────────

    #[test]
    fn decision_serde_snake_case() {
        for (variant, expected) in [
            (Decision::Approve, "\"approve\""),
            (Decision::Deny, "\"deny\""),
            (Decision::Ask, "\"ask\""),
        ] {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(serialized, expected);
            let deserialized: Decision = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, variant);
        }
    }

    // ── RichDecision ───────────────────────────────────────────────────

    #[test]
    fn rich_decision_approve_defaults() {
        let rd = RichDecision::approve();
        assert_eq!(rd.decision, Decision::Approve);
        assert!(rd.message.is_none());
        assert!(rd.updated_input.is_none());
        assert!(!rd.always_allow);
        assert!(rd.additional_context.is_none());
        assert!(rd.selected_permission.is_none());
    }

    #[test]
    fn rich_decision_deny_defaults() {
        let rd = RichDecision::deny();
        assert_eq!(rd.decision, Decision::Deny);
        assert!(rd.message.is_none());
        assert!(rd.updated_input.is_none());
        assert!(!rd.always_allow);
        assert!(rd.additional_context.is_none());
        assert!(rd.selected_permission.is_none());
    }

    #[test]
    fn rich_decision_from_decision() {
        for decision in [Decision::Approve, Decision::Deny, Decision::Ask] {
            let rd: RichDecision = decision.into();
            assert_eq!(rd.decision, decision);
            assert!(rd.message.is_none());
            assert!(rd.updated_input.is_none());
            assert!(!rd.always_allow);
            assert!(rd.additional_context.is_none());
            assert!(rd.selected_permission.is_none());
        }
    }

    // ── AutoApproveLevel ───────────────────────────────────────────────

    #[test]
    fn auto_approve_off_includes_nothing() {
        assert!(!AutoApproveLevel::Off.includes("Read"));
        assert!(!AutoApproveLevel::Off.includes("Bash"));
        assert!(!AutoApproveLevel::Off.includes("Edit"));
        assert!(!AutoApproveLevel::Off.includes("anything"));
    }

    #[test]
    fn auto_approve_read_includes_read_tools() {
        assert!(AutoApproveLevel::Read.includes("Read"));
        assert!(AutoApproveLevel::Read.includes("Grep"));
        assert!(AutoApproveLevel::Read.includes("Glob"));
        assert!(AutoApproveLevel::Read.includes("WebSearch"));
        assert!(!AutoApproveLevel::Read.includes("Edit"));
        assert!(!AutoApproveLevel::Read.includes("Bash"));
    }

    #[test]
    fn all_known_tools_derives_from_tiers_and_always_ask() {
        // itr#121: the TUI/CLI toggle lists are derived, not hardcoded. Every
        // tiered tool and every always-defer tool must appear exactly once.
        let all = all_known_tools();
        for tier in [
            AutoApproveLevel::Read,
            AutoApproveLevel::Write,
            AutoApproveLevel::Execute,
        ] {
            for tool in tier.tier_tools() {
                assert!(all.contains(tool), "{tool} missing from all_known_tools");
            }
        }
        for tool in DEFAULT_ALWAYS_ASK {
            assert!(all.contains(tool), "{tool} missing from all_known_tools");
        }
        // No duplicates — tiers are disjoint and disjoint from always-ask.
        let mut seen = std::collections::HashSet::new();
        for tool in &all {
            assert!(
                seen.insert(*tool),
                "duplicate tool {tool} in all_known_tools"
            );
        }
    }

    #[test]
    fn tool_tiers_mirror_the_levels() {
        // itr#121: the web view is the same data the levels enforce.
        let tiers = ToolTiers::current();
        assert_eq!(tiers.read, AutoApproveLevel::Read.tier_tools());
        assert_eq!(tiers.write, AutoApproveLevel::Write.tier_tools());
        assert_eq!(tiers.execute, AutoApproveLevel::Execute.tier_tools());
        assert_eq!(tiers.always_ask, DEFAULT_ALWAYS_ASK);
        // Serializes to the {read,write,execute,always_ask} shape the SPA reads.
        let json = serde_json::to_value(&tiers).unwrap();
        assert!(json.get("read").and_then(|v| v.as_array()).is_some());
        assert!(
            json["read"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "WebSearch")
        );
    }

    #[test]
    fn auto_approve_write_includes_read_and_write() {
        // Write tier tools
        assert!(AutoApproveLevel::Write.includes("Edit"));
        assert!(AutoApproveLevel::Write.includes("Write"));
        assert!(AutoApproveLevel::Write.includes("NotebookEdit"));
        // Read tier tools (inherited)
        assert!(AutoApproveLevel::Write.includes("Read"));
        assert!(AutoApproveLevel::Write.includes("Grep"));
        // Execute tier tools (not included)
        assert!(!AutoApproveLevel::Write.includes("Bash"));
    }

    #[test]
    fn auto_approve_execute_includes_all_below() {
        assert!(AutoApproveLevel::Execute.includes("Bash"));
        assert!(AutoApproveLevel::Execute.includes("Edit"));
        assert!(AutoApproveLevel::Execute.includes("Read"));
        assert!(AutoApproveLevel::Execute.includes("Write"));
        assert!(AutoApproveLevel::Execute.includes("Grep"));
    }

    #[test]
    fn auto_approve_all_includes_everything() {
        assert!(AutoApproveLevel::All.includes("anything"));
        assert!(AutoApproveLevel::All.includes("Bash"));
        assert!(AutoApproveLevel::All.includes("Read"));
        assert!(AutoApproveLevel::All.includes("SomeFutureTool"));
        assert!(AutoApproveLevel::All.includes(""));
    }

    #[test]
    fn auto_approve_from_str_names() {
        assert_eq!(
            "off".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Off
        );
        assert_eq!(
            "read".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Read
        );
        assert_eq!(
            "write".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Write
        );
        assert_eq!(
            "execute".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Execute
        );
        assert_eq!(
            "exec".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Execute
        );
        assert_eq!(
            "all".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::All
        );
    }

    #[test]
    fn auto_approve_from_str_numbers() {
        assert_eq!(
            "0".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Off
        );
        assert_eq!(
            "1".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Read
        );
        assert_eq!(
            "2".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Write
        );
        assert_eq!(
            "3".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Execute
        );
        assert_eq!(
            "4".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::All
        );
    }

    #[test]
    fn auto_approve_from_str_case_insensitive() {
        assert_eq!(
            "OFF".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Off
        );
        assert_eq!(
            "Read".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Read
        );
        assert_eq!(
            "WRITE".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Write
        );
        assert_eq!(
            "Execute".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::Execute
        );
        assert_eq!(
            "ALL".parse::<AutoApproveLevel>().unwrap(),
            AutoApproveLevel::All
        );
    }

    #[test]
    fn auto_approve_from_str_invalid() {
        assert!("bogus".parse::<AutoApproveLevel>().is_err());
        assert!("5".parse::<AutoApproveLevel>().is_err());
        assert!("".parse::<AutoApproveLevel>().is_err());
    }

    #[test]
    fn auto_approve_default_is_read() {
        assert_eq!(AutoApproveLevel::default(), AutoApproveLevel::Read);
    }

    #[test]
    fn auto_approve_display() {
        assert_eq!(AutoApproveLevel::Off.to_string(), "off");
        assert_eq!(AutoApproveLevel::Read.to_string(), "read");
        assert_eq!(AutoApproveLevel::Write.to_string(), "write");
        assert_eq!(AutoApproveLevel::Execute.to_string(), "execute");
        assert_eq!(AutoApproveLevel::All.to_string(), "all");
    }

    // ── DecisionFilter ─────────────────────────────────────────────────

    #[test]
    fn filter_empty_matches_everything() {
        let filter = DecisionFilter::default();
        let req = make_request("Bash", "agent-1", "/tmp/proj", AgentType::ClaudeCode);
        assert!(filter.matches(&req));

        let req2 = make_request("Edit", "agent-2", "/other/proj", AgentType::Red);
        assert!(filter.matches(&req2));
    }

    #[test]
    fn filter_tool_name_matches() {
        let filter = DecisionFilter {
            tool_name: Some("Bash".into()),
            ..Default::default()
        };
        let bash_req = make_request("Bash", "agent-1", "/proj", AgentType::ClaudeCode);
        assert!(filter.matches(&bash_req));

        let edit_req = make_request("Edit", "agent-1", "/proj", AgentType::ClaudeCode);
        assert!(!filter.matches(&edit_req));
    }

    #[test]
    fn filter_project_matches() {
        let filter = DecisionFilter {
            project: Some(PathBuf::from("/my/project")),
            ..Default::default()
        };
        let matching = make_request("Bash", "agent-1", "/my/project", AgentType::ClaudeCode);
        assert!(filter.matches(&matching));

        let non_matching = make_request("Bash", "agent-1", "/other/project", AgentType::ClaudeCode);
        assert!(!filter.matches(&non_matching));
    }

    #[test]
    fn filter_agent_type_matches() {
        let filter = DecisionFilter {
            agent_type: Some(AgentType::Red),
            ..Default::default()
        };
        let red_req = make_request("Bash", "agent-1", "/proj", AgentType::Red);
        assert!(filter.matches(&red_req));

        let claude_req = make_request("Bash", "agent-1", "/proj", AgentType::ClaudeCode);
        assert!(!filter.matches(&claude_req));
    }

    #[test]
    fn filter_multiple_fields_all_must_match() {
        let filter = DecisionFilter {
            tool_name: Some("Bash".into()),
            project: Some(PathBuf::from("/my/project")),
            agent_type: None,
        };

        // Both match
        let full_match = make_request("Bash", "agent-1", "/my/project", AgentType::ClaudeCode);
        assert!(filter.matches(&full_match));

        // Tool matches, project doesn't
        let tool_only = make_request("Bash", "agent-1", "/other/project", AgentType::ClaudeCode);
        assert!(!filter.matches(&tool_only));

        // Project matches, tool doesn't
        let project_only = make_request("Edit", "agent-1", "/my/project", AgentType::ClaudeCode);
        assert!(!filter.matches(&project_only));

        // Neither matches
        let neither = make_request("Edit", "agent-1", "/other/project", AgentType::ClaudeCode);
        assert!(!filter.matches(&neither));
    }
}
