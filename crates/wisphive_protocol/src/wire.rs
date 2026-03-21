use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{AgentInfo, Decision, DecisionFilter, DecisionRequest, HistoryEntry, HistorySearch, ManagedAgent, SpawnAgentRequest, ToolResult};

/// Identifies the type of client connecting to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientType {
    Hook,
    Tui,
}

/// Messages sent from clients (hook or TUI) to the daemon.
///
/// Wire format: newline-delimited JSON over Unix socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Initial handshake — must be the first message on any connection.
    #[serde(rename = "hello")]
    Hello { client: ClientType, version: u32 },

    /// Hook submits a tool call for human decision. Hook blocks until response.
    #[serde(rename = "decision_request")]
    DecisionRequest(DecisionRequest),

    /// TUI approves a single queued decision (with optional rich fields).
    #[serde(rename = "approve")]
    Approve {
        id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        updated_input: Option<serde_json::Value>,
        #[serde(default)]
        always_allow: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        additional_context: Option<String>,
    },

    /// TUI denies a single queued decision (with optional feedback).
    #[serde(rename = "deny")]
    Deny {
        id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
    },

    /// TUI defers to the agent's native permission prompt.
    #[serde(rename = "ask")]
    Ask { id: Uuid },

    /// TUI approves all items matching an optional filter.
    #[serde(rename = "approve_all")]
    ApproveAll { filter: Option<DecisionFilter> },

    /// TUI denies all items matching an optional filter.
    #[serde(rename = "deny_all")]
    DenyAll { filter: Option<DecisionFilter> },

    /// Request the daemon to spawn a new agent process.
    #[serde(rename = "spawn_agent")]
    SpawnAgent(SpawnAgentRequest),

    /// List all daemon-managed agent processes.
    #[serde(rename = "list_agents")]
    ListAgents,

    /// Stop a daemon-managed agent process.
    #[serde(rename = "stop_agent")]
    StopAgent { agent_id: String },

    /// Query decision history from the audit log.
    #[serde(rename = "query_history")]
    QueryHistory {
        /// Filter by agent ID. None = all agents.
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// Maximum number of entries to return (default 200).
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    /// Hook reports a tool execution result (fire-and-forget, PostToolUse).
    #[serde(rename = "tool_result")]
    ToolResult(ToolResult),

    /// Search decision history with rich filters.
    #[serde(rename = "search_history")]
    SearchHistory(HistorySearch),
}

/// Messages sent from the daemon to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Handshake response.
    #[serde(rename = "welcome")]
    Welcome { version: u32 },

    /// Response to a hook's DecisionRequest (with optional rich fields).
    #[serde(rename = "decision_response")]
    DecisionResponse {
        id: Uuid,
        decision: Decision,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        updated_input: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        additional_context: Option<String>,
    },

    /// Full queue snapshot sent to TUI on connect.
    #[serde(rename = "queue_snapshot")]
    QueueSnapshot { items: Vec<DecisionRequest> },

    /// A new decision has been queued.
    #[serde(rename = "new_decision")]
    NewDecision(DecisionRequest),

    /// A decision was resolved (approved or denied).
    #[serde(rename = "decision_resolved")]
    DecisionResolved { id: Uuid, decision: Decision },

    /// An agent connected (new hook session started).
    #[serde(rename = "agent_connected")]
    AgentConnected(AgentInfo),

    /// An agent disconnected.
    #[serde(rename = "agent_disconnected")]
    AgentDisconnected { agent_id: String },

    /// A managed agent process was spawned by the daemon.
    #[serde(rename = "agent_spawned")]
    AgentSpawned(ManagedAgent),

    /// A managed agent process exited.
    #[serde(rename = "agent_exited")]
    AgentExited {
        agent_id: String,
        exit_code: Option<i32>,
    },

    /// Response to ListAgents request.
    #[serde(rename = "agent_list")]
    AgentList { agents: Vec<ManagedAgent> },

    /// Response to QueryHistory request.
    #[serde(rename = "history_response")]
    HistoryResponse { entries: Vec<HistoryEntry> },

    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },
}

/// Protocol version. Increment on breaking wire format changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Serialize a message to a newline-terminated JSON string.
pub fn encode<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string(msg)?;
    json.push('\n');
    Ok(json)
}

/// Deserialize a message from a JSON string (newline is optional on input).
pub fn decode<'a, T: Deserialize<'a>>(json: &'a str) -> Result<T, serde_json::Error> {
    serde_json::from_str(json.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentType;
    use std::path::PathBuf;

    #[test]
    fn round_trip_hello() {
        let msg = ClientMessage::Hello {
            client: ClientType::Hook,
            version: PROTOCOL_VERSION,
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.ends_with('\n'));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Hello { client, version } => {
                assert_eq!(client, ClientType::Hook);
                assert_eq!(version, PROTOCOL_VERSION);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_decision_request() {
        let req = DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: "cc-1".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/Users/test/project"),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "cargo build"}),
            timestamp: chrono::Utc::now(),
        };
        let msg = ClientMessage::DecisionRequest(req);
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::DecisionRequest(r) => {
                assert_eq!(r.tool_name, "Bash");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn filter_matches() {
        let req = DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: "cc-1".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/muse"),
            tool_name: "Bash".into(),
            tool_input: serde_json::Value::Null,
            timestamp: chrono::Utc::now(),
        };

        let filter = DecisionFilter {
            tool_name: Some("Bash".into()),
            ..Default::default()
        };
        assert!(filter.matches(&req));

        let filter = DecisionFilter {
            tool_name: Some("Write".into()),
            ..Default::default()
        };
        assert!(!filter.matches(&req));
    }
}
