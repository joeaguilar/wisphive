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
}

impl RichDecision {
    pub fn approve() -> Self {
        Self {
            decision: Decision::Approve,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
        }
    }

    pub fn deny() -> Self {
        Self {
            decision: Decision::Deny,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
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
        }
    }
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
