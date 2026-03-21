use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The type of agent connected to Wisphive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    ClaudeCode,
    Red,
    LocalLlm,
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaudeCode => write!(f, "claude_code"),
            Self::Red => write!(f, "red"),
            Self::LocalLlm => write!(f, "local_llm"),
        }
    }
}

/// Metadata about a connected agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub started_at: DateTime<Utc>,
}

/// A permission rule from Claude Code's PermissionRequest event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRule {
    #[serde(rename = "toolName")]
    pub tool_name: String,
    #[serde(rename = "ruleContent")]
    pub rule_content: String,
}

/// A permission suggestion from Claude Code's PermissionRequest event.
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

/// A decision the human needs to make about a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub id: Uuid,
    pub agent_id: String,
    pub agent_type: AgentType,
    pub project: PathBuf,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    /// Permission suggestions from Claude Code's PermissionRequest event.
    /// Present only for PermissionRequest hooks (not PreToolUse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_suggestions: Option<Vec<PermissionSuggestion>>,
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

/// Request to spawn a new AI agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
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
}

/// Metadata about a daemon-managed agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedAgent {
    pub agent_id: String,
    pub pid: u32,
    pub project: PathBuf,
    pub model: Option<String>,
    pub name: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// A tool execution result reported by the PostToolUse hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub agent_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_result: serde_json::Value,
    pub timestamp: DateTime<Utc>,
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
pub enum AutoApproveLevel {
    /// Nothing auto-approved. Every tool goes through the TUI.
    Off,
    /// Read-only + orchestration tools (default).
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
    fn tier_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::Read => &[
                // File/content reading
                "Read", "Glob", "Grep", "LS", "LSP", "NotebookRead",
                // Web (read-only)
                "WebSearch", "WebFetch",
                // Orchestration & planning
                "Agent", "Skill", "ToolSearch", "AskUserQuestion",
                "EnterPlanMode", "ExitPlanMode",
                "EnterWorktree", "ExitWorktree",
                // Task management
                "TaskCreate", "TaskUpdate", "TaskGet", "TaskList",
                "TaskOutput", "TaskStop", "TodoRead",
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
            if level.tier_tools().iter().any(|&t| t == tool_name) {
                return true;
            }
        }
        false
    }
}

impl Default for AutoApproveLevel {
    fn default() -> Self {
        Self::Read
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

impl std::str::FromStr for AutoApproveLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" | "0" => Ok(Self::Off),
            "read" | "1" => Ok(Self::Read),
            "write" | "2" => Ok(Self::Write),
            "execute" | "exec" | "3" => Ok(Self::Execute),
            "all" | "4" => Ok(Self::All),
            _ => Err(format!("unknown level: {s}. Valid: off, read, write, execute, all")),
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
        if let Some(ref tool) = self.tool_name {
            if req.tool_name != *tool {
                return false;
            }
        }
        if let Some(ref project) = self.project {
            if req.project != *project {
                return false;
            }
        }
        if let Some(ref agent_type) = self.agent_type {
            if req.agent_type != *agent_type {
                return false;
            }
        }
        true
    }
}
