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
