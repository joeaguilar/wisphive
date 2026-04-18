use serde::{Deserialize, Serialize};
use uuid::Uuid;

use std::path::PathBuf;

use std::collections::HashMap;

use crate::types::{
    AgentInfo, AgentType, Decision, DecisionFilter, DecisionRequest, HistoryEntry, HistorySearch,
    ManagedAgent, PermissionSuggestion, ProjectSummary, SessionSummary, SpawnAgentRequest,
    TerminalDirection, TerminalSessionMeta, TerminalStatus, ToolResult,
};

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
        /// Opaque correlation ID echoed back in the response.
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    /// Hook reports a tool execution result (fire-and-forget, PostToolUse).
    #[serde(rename = "tool_result")]
    ToolResult(ToolResult),

    /// Search decision history with rich filters.
    #[serde(rename = "search_history")]
    SearchHistory(HistorySearch),

    /// Query session summaries (live + historical).
    #[serde(rename = "query_sessions")]
    QuerySessions,

    /// Query project summaries (aggregated across all agents).
    #[serde(rename = "query_projects")]
    QueryProjects,

    /// Hook registers an agent session (fire-and-forget, no response expected).
    #[serde(rename = "agent_register")]
    AgentRegister {
        agent_id: String,
        agent_type: AgentType,
        project: PathBuf,
    },

    /// Request a full reimport of events.jsonl into the history database.
    #[serde(rename = "reimport_events")]
    ReimportEvents,

    /// TUI approves a PermissionRequest with a specific suggestion selected.
    #[serde(rename = "approve_permission")]
    ApprovePermission {
        id: Uuid,
        /// Index into the DecisionRequest's permission_suggestions array.
        suggestion_index: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
    },

    // ── Terminal sessions ─────────────────────────────────────────────
    /// Create a new daemon-managed PTY session.
    #[serde(rename = "term_create")]
    TermCreate {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        label: Option<String>,
        /// Command to spawn. None = user's login shell (`$SHELL -l`).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        command: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        args: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        /// Extra env vars merged into the child's environment.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        env: Option<HashMap<String, String>>,
    },

    /// Attach to an existing terminal session to receive live output.
    /// Daemon replies with a `TermCatchup` snapshot followed by ongoing `TermChunk`s.
    #[serde(rename = "term_attach")]
    TermAttach { id: Uuid },

    /// Detach from a terminal session (stop receiving its output on this connection).
    #[serde(rename = "term_detach")]
    TermDetach { id: Uuid },

    /// Forward bytes to the PTY's stdin. `data` is base64-encoded.
    #[serde(rename = "term_input")]
    TermInput { id: Uuid, data: String },

    /// Resize the PTY window.
    #[serde(rename = "term_resize")]
    TermResize { id: Uuid, cols: u16, rows: u16 },

    /// Close a terminal session. If `kill` is true, send SIGKILL to the child.
    #[serde(rename = "term_close")]
    TermClose {
        id: Uuid,
        #[serde(default)]
        kill: bool,
    },

    /// List all terminal sessions (running + historical).
    #[serde(rename = "term_list")]
    TermList,

    /// Replay a session's event history as a stream of `TermReplayChunk`s.
    #[serde(rename = "term_replay")]
    TermReplay {
        id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        from_seq: Option<u64>,
        /// Playback speed multiplier; clients pace the writes client-side.
        /// Passed through unchanged so the daemon can skip pacing server-side.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        speed: Option<f32>,
    },
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
        /// Selected permission suggestion (PermissionRequest responses only).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        selected_permission: Option<PermissionSuggestion>,
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
    HistoryResponse {
        entries: Vec<HistoryEntry>,
        /// Echoed correlation ID from the request.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        request_id: Option<String>,
    },

    /// Response to QuerySessions: list of session summaries.
    #[serde(rename = "sessions_response")]
    SessionsResponse { sessions: Vec<SessionSummary> },

    /// Response to QueryProjects: list of project summaries.
    #[serde(rename = "projects_response")]
    ProjectsResponse { projects: Vec<ProjectSummary> },

    /// Full snapshot of currently registered agents, sent to TUI on connect.
    #[serde(rename = "agents_snapshot")]
    AgentsSnapshot { agents: Vec<AgentInfo> },

    /// Response to ReimportEvents: how many events were imported.
    #[serde(rename = "reimport_complete")]
    ReimportComplete { count: u64 },

    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },

    // ── Terminal sessions ─────────────────────────────────────────────
    /// Confirms a terminal session was created and delivers its metadata.
    #[serde(rename = "term_created")]
    TermCreated(TerminalSessionMeta),

    /// Response to `TermList`.
    #[serde(rename = "term_list_response")]
    TermListResponse { sessions: Vec<TerminalSessionMeta> },

    /// A live chunk of bytes from a terminal. `data` is base64-encoded.
    #[serde(rename = "term_chunk")]
    TermChunk {
        id: Uuid,
        seq: u64,
        ts_us: i64,
        direction: TerminalDirection,
        data: String,
    },

    /// Catchup snapshot sent when a client attaches to a running session.
    /// `screen` is a base64-encoded vt100 `contents_formatted()` buffer that,
    /// when written to a terminal emulator, reproduces the current screen state.
    #[serde(rename = "term_catchup")]
    TermCatchup {
        id: Uuid,
        cols: u16,
        rows: u16,
        /// The sequence number of the next live chunk the viewer will receive.
        next_seq: u64,
        screen: String,
    },

    /// A terminal session has ended (cleanly, killed, or orphaned).
    #[serde(rename = "term_ended")]
    TermEnded {
        id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        exit_code: Option<i32>,
        status: TerminalStatus,
    },

    /// A single event from a replay stream. `data` is base64-encoded.
    #[serde(rename = "term_replay_chunk")]
    TermReplayChunk {
        id: Uuid,
        seq: u64,
        ts_us: i64,
        direction: TerminalDirection,
        data: String,
    },

    /// Signals the end of a replay stream.
    #[serde(rename = "term_replay_done")]
    TermReplayDone { id: Uuid, total_events: u64 },

    /// A terminal-specific error (session not found, lagged, create failed, etc.).
    #[serde(rename = "term_error")]
    TermError {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        id: Option<Uuid>,
        message: String,
    },
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
            hook_event_name: Default::default(),
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
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
            hook_event_name: Default::default(),
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
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

    // ── Server messages ──────────────────────────────────────────────

    #[test]
    fn round_trip_welcome() {
        let msg = ServerMessage::Welcome { version: PROTOCOL_VERSION };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Welcome { version } => assert_eq!(version, PROTOCOL_VERSION),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_decision_response() {
        let id = uuid::Uuid::new_v4();
        let msg = ServerMessage::DecisionResponse {
            id,
            decision: Decision::Approve,
            message: Some("looks good".into()),
            updated_input: Some(serde_json::json!({"command": "cargo test"})),
            additional_context: Some("run tests first".into()),
            selected_permission: None,
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::DecisionResponse {
                id: did,
                decision,
                message,
                updated_input,
                additional_context,
                ..
            } => {
                assert_eq!(did, id);
                assert_eq!(decision, Decision::Approve);
                assert_eq!(message.unwrap(), "looks good");
                assert_eq!(updated_input.unwrap(), serde_json::json!({"command": "cargo test"}));
                assert_eq!(additional_context.unwrap(), "run tests first");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_queue_snapshot() {
        let req = DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: "cc-2".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/tmp/proj"),
            tool_name: "Write".into(),
            tool_input: serde_json::json!({"path": "/tmp/proj/foo.rs"}),
            timestamp: chrono::Utc::now(),
            hook_event_name: Default::default(),
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
        };
        let msg = ServerMessage::QueueSnapshot { items: vec![req] };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::QueueSnapshot { items } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].tool_name, "Write");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_error() {
        let msg = ServerMessage::Error { message: "something went wrong".into() };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Error { message } => assert_eq!(message, "something went wrong"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_decision_resolved() {
        let id = uuid::Uuid::new_v4();
        let msg = ServerMessage::DecisionResolved { id, decision: Decision::Deny };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::DecisionResolved { id: did, decision } => {
                assert_eq!(did, id);
                assert_eq!(decision, Decision::Deny);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_reimport_complete() {
        let msg = ServerMessage::ReimportComplete { count: 42 };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::ReimportComplete { count } => assert_eq!(count, 42),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_history_response() {
        use crate::types::HistoryEntry;
        let entry = HistoryEntry {
            id: uuid::Uuid::new_v4(),
            agent_id: "cc-1".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/proj"),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            decision: Decision::Approve,
            requested_at: chrono::Utc::now(),
            resolved_at: chrono::Utc::now(),
            tool_result: None,
            tool_use_id: None,
            hook_event_name: None,
            terminal_session_id: None,
        };
        let msg = ServerMessage::HistoryResponse {
            entries: vec![entry],
            request_id: Some("req-123".into()),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::HistoryResponse { entries, request_id } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].tool_name, "Bash");
                assert_eq!(request_id.unwrap(), "req-123");
            }
            _ => panic!("unexpected variant"),
        }
    }

    // ── Client messages ──────────────────────────────────────────────

    #[test]
    fn round_trip_approve() {
        let id = uuid::Uuid::new_v4();
        let msg = ClientMessage::Approve {
            id,
            message: Some("approved with edits".into()),
            updated_input: Some(serde_json::json!({"command": "cargo build --release"})),
            always_allow: true,
            additional_context: Some("use release mode".into()),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Approve {
                id: did,
                message,
                updated_input,
                always_allow,
                additional_context,
            } => {
                assert_eq!(did, id);
                assert_eq!(message.unwrap(), "approved with edits");
                assert_eq!(
                    updated_input.unwrap(),
                    serde_json::json!({"command": "cargo build --release"})
                );
                assert!(always_allow);
                assert_eq!(additional_context.unwrap(), "use release mode");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_deny() {
        let id = uuid::Uuid::new_v4();
        let msg = ClientMessage::Deny {
            id,
            message: Some("too dangerous".into()),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Deny { id: did, message } => {
                assert_eq!(did, id);
                assert_eq!(message.unwrap(), "too dangerous");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_ask() {
        let id = uuid::Uuid::new_v4();
        let msg = ClientMessage::Ask { id };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Ask { id: did } => assert_eq!(did, id),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_approve_all_with_filter() {
        let msg = ClientMessage::ApproveAll {
            filter: Some(DecisionFilter {
                tool_name: Some("Bash".into()),
                project: Some(PathBuf::from("/proj")),
                agent_type: Some(AgentType::ClaudeCode),
            }),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::ApproveAll { filter } => {
                let f = filter.unwrap();
                assert_eq!(f.tool_name.unwrap(), "Bash");
                assert_eq!(f.project.unwrap(), PathBuf::from("/proj"));
                assert_eq!(f.agent_type.unwrap(), AgentType::ClaudeCode);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_deny_all_no_filter() {
        let msg = ClientMessage::DenyAll { filter: None };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::DenyAll { filter } => assert!(filter.is_none()),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_query_history() {
        let msg = ClientMessage::QueryHistory {
            agent_id: Some("cc-5".into()),
            limit: Some(50),
            request_id: Some("qh-1".into()),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::QueryHistory { agent_id, limit, request_id } => {
                assert_eq!(agent_id.unwrap(), "cc-5");
                assert_eq!(limit.unwrap(), 50);
                assert_eq!(request_id.unwrap(), "qh-1");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_search_history() {
        use crate::types::HistorySearch;
        let search = HistorySearch {
            query: Some("cargo".into()),
            tool_name: Some("Bash".into()),
            agent_id: None,
            limit: Some(10),
            request_id: Some("sh-1".into()),
        };
        let msg = ClientMessage::SearchHistory(search);
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::SearchHistory(s) => {
                assert_eq!(s.query.unwrap(), "cargo");
                assert_eq!(s.tool_name.unwrap(), "Bash");
                assert!(s.agent_id.is_none());
                assert_eq!(s.limit.unwrap(), 10);
                assert_eq!(s.request_id.unwrap(), "sh-1");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_query_sessions() {
        let msg = ClientMessage::QuerySessions;
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ClientMessage::QuerySessions));
    }

    #[test]
    fn round_trip_query_projects() {
        let msg = ClientMessage::QueryProjects;
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ClientMessage::QueryProjects));
    }

    #[test]
    fn round_trip_reimport_events() {
        let msg = ClientMessage::ReimportEvents;
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ClientMessage::ReimportEvents));
    }

    #[test]
    fn round_trip_agent_register() {
        let msg = ClientMessage::AgentRegister {
            agent_id: "cc-99".into(),
            agent_type: AgentType::Red,
            project: PathBuf::from("/home/user/project"),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::AgentRegister { agent_id, agent_type, project } => {
                assert_eq!(agent_id, "cc-99");
                assert_eq!(agent_type, AgentType::Red);
                assert_eq!(project, PathBuf::from("/home/user/project"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_tool_result() {
        use crate::types::ToolResult;
        let tr = ToolResult {
            agent_id: "cc-1".into(),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "echo hi"}),
            tool_result: serde_json::json!({"stdout": "hi\n", "exit_code": 0}),
            timestamp: chrono::Utc::now(),
            tool_use_id: Some("tu-abc".into()),
        };
        let msg = ClientMessage::ToolResult(tr);
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::ToolResult(r) => {
                assert_eq!(r.agent_id, "cc-1");
                assert_eq!(r.tool_name, "Bash");
                assert_eq!(r.tool_use_id.unwrap(), "tu-abc");
            }
            _ => panic!("unexpected variant"),
        }
    }

    // ── Encoding edge cases ──────────────────────────────────────────

    #[test]
    fn encode_appends_newline() {
        let msg = ServerMessage::Welcome { version: 1 };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.ends_with('\n'));
        // Exactly one trailing newline
        assert!(!encoded.ends_with("\n\n"));
    }

    #[test]
    fn decode_strips_whitespace() {
        let msg = ClientMessage::Hello {
            client: ClientType::Tui,
            version: PROTOCOL_VERSION,
        };
        let encoded = encode(&msg).unwrap();
        // Wrap with leading/trailing spaces and newlines
        let padded = format!("  \n  {}  \n  ", encoded.trim());
        let decoded: ClientMessage = decode(&padded).unwrap();
        match decoded {
            ClientMessage::Hello { client, version } => {
                assert_eq!(client, ClientType::Tui);
                assert_eq!(version, PROTOCOL_VERSION);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn decode_invalid_json_returns_error() {
        let result = decode::<ClientMessage>("this is not json");
        assert!(result.is_err());
    }

    // ── Terminal session messages ────────────────────────────────────

    fn sample_meta() -> TerminalSessionMeta {
        TerminalSessionMeta {
            id: uuid::Uuid::new_v4(),
            label: Some("main".into()),
            command: "/bin/zsh".into(),
            args: vec!["-l".into()],
            cwd: PathBuf::from("/tmp/proj"),
            cols: 80,
            rows: 24,
            started_at: chrono::Utc::now(),
            ended_at: None,
            exit_code: None,
            status: TerminalStatus::Running,
        }
    }

    #[test]
    fn round_trip_term_create() {
        let msg = ClientMessage::TermCreate {
            label: Some("shell".into()),
            command: None,
            args: None,
            cwd: Some(PathBuf::from("/tmp")),
            cols: 120,
            rows: 40,
            env: None,
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"term_create\""));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::TermCreate { label, cols, rows, .. } => {
                assert_eq!(label.unwrap(), "shell");
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_term_input_with_control_bytes() {
        use base64::Engine;
        // Tab, newline, carriage return, Ctrl-C, high byte (Latin-1 é), UTF-8 snowman prefix
        let raw: &[u8] = &[0x09, 0x0a, 0x0d, 0x03, 0xe9, 0xe2, 0x98, 0x83];
        let encoded_payload = base64::engine::general_purpose::STANDARD.encode(raw);
        let id = uuid::Uuid::new_v4();
        let msg = ClientMessage::TermInput { id, data: encoded_payload.clone() };
        let line = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&line).unwrap();
        match decoded {
            ClientMessage::TermInput { id: did, data } => {
                assert_eq!(did, id);
                let round_tripped =
                    base64::engine::general_purpose::STANDARD.decode(&data).unwrap();
                assert_eq!(round_tripped, raw);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_term_attach_detach_close_list_replay() {
        let id = uuid::Uuid::new_v4();
        for msg in [
            ClientMessage::TermAttach { id },
            ClientMessage::TermDetach { id },
            ClientMessage::TermClose { id, kill: true },
            ClientMessage::TermList,
            ClientMessage::TermReplay { id, from_seq: Some(42), speed: Some(2.0) },
            ClientMessage::TermResize { id, cols: 100, rows: 30 },
        ] {
            let encoded = encode(&msg).unwrap();
            let _: ClientMessage = decode(&encoded).unwrap();
        }
    }

    #[test]
    fn round_trip_term_created_and_list_response() {
        let meta = sample_meta();
        let created = ServerMessage::TermCreated(meta.clone());
        let encoded = encode(&created).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::TermCreated(m) => {
                assert_eq!(m.command, "/bin/zsh");
                assert_eq!(m.status, TerminalStatus::Running);
            }
            _ => panic!("unexpected variant"),
        }
        let list = ServerMessage::TermListResponse { sessions: vec![meta] };
        let encoded = encode(&list).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ServerMessage::TermListResponse { .. }));
    }

    #[test]
    fn round_trip_term_chunk_and_catchup() {
        use base64::Engine;
        let id = uuid::Uuid::new_v4();
        // Payload with embedded newlines must survive JSON encoding.
        let raw = b"hello\nworld\r\n\x1b[31mred\x1b[0m";
        let data = base64::engine::general_purpose::STANDARD.encode(raw);
        let chunk = ServerMessage::TermChunk {
            id,
            seq: 1,
            ts_us: 123_456_789,
            direction: TerminalDirection::Output,
            data: data.clone(),
        };
        let encoded = encode(&chunk).unwrap();
        assert_eq!(encoded.matches('\n').count(), 1, "encoding must contain exactly one newline");
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::TermChunk { direction, data: d, .. } => {
                assert_eq!(direction, TerminalDirection::Output);
                let bytes = base64::engine::general_purpose::STANDARD.decode(&d).unwrap();
                assert_eq!(bytes, raw);
            }
            _ => panic!("unexpected variant"),
        }

        let catchup = ServerMessage::TermCatchup {
            id,
            cols: 80,
            rows: 24,
            next_seq: 5,
            screen: data,
        };
        let encoded = encode(&catchup).unwrap();
        let _: ServerMessage = decode(&encoded).unwrap();
    }

    #[test]
    fn round_trip_term_ended_and_error() {
        let id = uuid::Uuid::new_v4();
        let ended = ServerMessage::TermEnded {
            id,
            exit_code: Some(0),
            status: TerminalStatus::Exited,
        };
        let encoded = encode(&ended).unwrap();
        let _: ServerMessage = decode(&encoded).unwrap();

        let err = ServerMessage::TermError {
            id: Some(id),
            message: "session not found".into(),
        };
        let encoded = encode(&err).unwrap();
        let _: ServerMessage = decode(&encoded).unwrap();
    }

    #[test]
    fn round_trip_term_replay_chunk_and_done() {
        let id = uuid::Uuid::new_v4();
        let chunk = ServerMessage::TermReplayChunk {
            id,
            seq: 7,
            ts_us: 1_000,
            direction: TerminalDirection::Input,
            data: "aGVsbG8=".into(),
        };
        let encoded = encode(&chunk).unwrap();
        let _: ServerMessage = decode(&encoded).unwrap();

        let done = ServerMessage::TermReplayDone { id, total_events: 42 };
        let encoded = encode(&done).unwrap();
        let _: ServerMessage = decode(&encoded).unwrap();
    }

    #[test]
    fn round_trip_decision_request_with_terminal_session_id() {
        let term_id = uuid::Uuid::new_v4();
        let req = DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: "cc-1".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/proj"),
            tool_name: "Bash".into(),
            tool_input: serde_json::Value::Null,
            timestamp: chrono::Utc::now(),
            hook_event_name: Default::default(),
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: Some(term_id),
        };
        let encoded = encode(&ClientMessage::DecisionRequest(req)).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::DecisionRequest(r) => {
                assert_eq!(r.terminal_session_id, Some(term_id));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn terminal_status_and_direction_display_round_trip() {
        use std::str::FromStr;
        for status in [
            TerminalStatus::Running,
            TerminalStatus::Exited,
            TerminalStatus::Killed,
            TerminalStatus::Orphaned,
        ] {
            let parsed = TerminalStatus::from_str(&status.to_string()).unwrap();
            assert_eq!(parsed, status);
        }
        for dir in [
            TerminalDirection::Input,
            TerminalDirection::Output,
            TerminalDirection::Resize,
        ] {
            let parsed = TerminalDirection::from_str(&dir.to_string()).unwrap();
            assert_eq!(parsed, dir);
        }
    }

    #[test]
    fn tag_based_discrimination() {
        let client_msg = ClientMessage::Hello {
            client: ClientType::Hook,
            version: PROTOCOL_VERSION,
        };
        let server_msg = ServerMessage::Welcome { version: PROTOCOL_VERSION };

        let client_json = encode(&client_msg).unwrap();
        let server_json = encode(&server_msg).unwrap();

        // Both are valid JSON — verify they contain the right "type" tag
        assert!(client_json.contains("\"type\":\"hello\""));
        assert!(server_json.contains("\"type\":\"welcome\""));

        // Each decodes to the correct variant of its own enum
        let decoded_client: ClientMessage = decode(&client_json).unwrap();
        assert!(matches!(decoded_client, ClientMessage::Hello { .. }));

        let decoded_server: ServerMessage = decode(&server_json).unwrap();
        assert!(matches!(decoded_server, ServerMessage::Welcome { .. }));

        // Cross-decoding should fail (hello is not a ServerMessage variant)
        let cross_result = decode::<ServerMessage>(&client_json);
        assert!(cross_result.is_err());
    }
}
