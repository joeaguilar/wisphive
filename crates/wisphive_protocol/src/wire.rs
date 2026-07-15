use serde::{Deserialize, Serialize};
use uuid::Uuid;

use std::path::PathBuf;

use std::collections::HashMap;

use crate::types::{
    AgentInfo, AgentType, ArtifactTouch, AuditDecision, Decision, DecisionFilter, DecisionRequest,
    HistoryEntry, HistorySearch, ManagedAgent, PermissionSuggestion, ProjectHookStatus,
    ProjectSummary, SessionSummary, SpawnAgentRequest, TerminalDirection, TerminalSessionMeta,
    TerminalStatus, ToolResult, WorktreeStatus,
};

/// Identifies the type of client connecting to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientType {
    Hook,
    Tui,
}

/// Stable identifier for an authenticated web device (phone, tablet, laptop).
///
/// Newtype so it can't be accidentally swapped with other stringly-typed
/// ids floating around the protocol (`agent_id`, `tool_use_id`, `session_id`).
/// Wire format is a bare string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for DeviceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for DeviceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
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
    ///
    /// Originating device id travels on the outer [`ClientCommand`] envelope, not
    /// on the variant — see crate-level docs on `ClientCommand` for the rationale.
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
    ApproveAll {
        filter: Option<DecisionFilter>,
        /// Explicit confirmation for an UNFILTERED bulk approve (itr#88): the
        /// daemon rejects `filter: None` without it, so a compromised or buggy
        /// client can't blanket-approve everything with one message. Old
        /// clients decode as `false` (serde default) and are rejected.
        #[serde(default)]
        confirm: bool,
    },

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

    /// Query working-tree status for the daemon's active projects (itr#401,
    /// Command Center spec §5.3). Strictly read-only daemon-side: the probe
    /// runs only non-mutating git commands (`status` / `diff`) and never
    /// stages, commits, or otherwise writes — the strip is a state mirror.
    #[serde(rename = "query_worktrees")]
    QueryWorktrees,

    /// Query recent APPROVED artifact-candidate tool calls for the burn meter
    /// (itr#402, Command Center spec §5.4). Strictly read-only daemon-side —
    /// one windowed SELECT over `decision_log`; the meter is a state mirror
    /// that never stops, throttles, or retargets anything. Answered with a
    /// [`ServerMessage::BurnResponse`].
    #[serde(rename = "query_burn")]
    QueryBurn,

    /// Web UI asks the daemon to install Wisphive hooks into `project`
    /// (itr#460). Sudo-class: the write lands in the project's
    /// `.claude/settings.json` / `.codex/hooks.json`, so the daemon requires a
    /// fresh web reauth (the originating `device_id` travels on the
    /// [`ClientCommand`] envelope). TUI origins bypass the gate.
    #[serde(rename = "install_hooks")]
    InstallHooks { project: PathBuf },

    /// Read-only query for `project`'s current Wisphive hook install status.
    /// No gate — returns a [`ServerMessage::ProjectHookStatus`].
    #[serde(rename = "query_project_hook_status")]
    QueryProjectHookStatus { project: PathBuf },

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

    /// Close a terminal session using the daemon's single platform-defined
    /// termination behavior.
    #[serde(rename = "term_close")]
    TermClose { id: Uuid },

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

    /// Assign a group label to a terminal session (None clears to ungrouped).
    /// Groups are purely organizational — purely a sidebar display hint.
    #[serde(rename = "term_set_group")]
    TermSetGroup {
        id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        group: Option<String>,
    },

    /// Update a terminal session's manual sort order. Smaller values sort first
    /// within a group. Clients use fractional indexing (midpoint between
    /// neighbors) to avoid rewriting sibling rows on each drag.
    #[serde(rename = "term_reorder")]
    TermReorder { id: Uuid, sort_order: i64 },

    /// Mark the originating web device as freshly reauthenticated, resetting
    /// its sudo-mode TTL. Emitted by `wisphive_web::post_auth_reauth` after
    /// a successful password re-entry; the daemon reads `device_id` from the
    /// [`ClientCommand`] envelope (never from the payload) and touches its
    /// reauth registry. See `wisphive_daemon::sudo_gate` for the policy.
    ///
    /// No body fields: there's nothing the client needs to say beyond "I
    /// just reauthed." Device attribution is end-to-end spoof-proof because
    /// the envelope's `device_id` is set by `ws_bridge::rewrap_with_device`
    /// / the reauth route's short-lived sender — the browser can't forge it.
    #[serde(rename = "mark_device_fresh")]
    MarkDeviceFresh,
}

/// Envelope wrapping a [`ClientMessage`] with per-connection client context.
///
/// Client context (currently just `device_id`; eventually IP, session id, UA
/// hash) lives on the envelope so new decision variants don't have to remember
/// to carry each field individually. The body is `#[serde(flatten)]`d, so the
/// wire format is byte-identical to a bare `ClientMessage` — decoders that
/// used to target `ClientMessage` can switch to `ClientCommand` and old
/// payloads (including ones that embedded `device_id` on the variant) continue
/// to decode cleanly: the top-level envelope absorbs the field.
///
/// Callers that don't need client context can keep encoding a `ClientMessage`
/// directly — the bytes match `ClientCommand { body, device_id: None }`
/// because `skip_serializing_if = "Option::is_none"` elides the field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCommand {
    #[serde(flatten)]
    pub body: ClientMessage,
    /// Originating web device id. None for TUI/local connections (implicitly
    /// trusted) and for non-decision variants that have no actor attribution.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub device_id: Option<DeviceId>,
    /// Opaque correlation ID for a one-shot command response.  It is optional
    /// so existing clients and daemons continue to decode bare commands.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub correlation_id: Option<String>,
}

impl ClientCommand {
    pub fn new(body: ClientMessage) -> Self {
        Self {
            body,
            device_id: None,
            correlation_id: None,
        }
    }

    pub fn with_device_id(mut self, device_id: DeviceId) -> Self {
        self.device_id = Some(device_id);
        self
    }

    pub fn with_correlation_id(mut self, correlation_id: String) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }
}

impl From<ClientMessage> for ClientCommand {
    fn from(body: ClientMessage) -> Self {
        Self::new(body)
    }
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

    /// Bounded recent audit snapshot sent to TUI/web clients on connect.
    #[serde(rename = "audit_snapshot")]
    AuditSnapshot { items: Vec<AuditDecision> },

    /// Live audit event for auto-answered/deferred/denied hook decisions.
    #[serde(rename = "audit_decision")]
    AuditDecision(AuditDecision),

    /// A previously DEFERRED native prompt (AskUserQuestion / ExitPlanMode / Elicitation)
    /// was ANSWERED in the agent's terminal. Emitted when the daemon correlates a
    /// PostToolUse `ToolResult` onto the deferred `decision_log` row (via `tool_use_id`).
    /// Lets the inbox clear the matching "waiting in your terminal" row; the outcome then
    /// surfaces in the audit/History feed (itr#440 / #461). `answer_summary` is a redacted,
    /// human-readable one-liner of the chosen option(s), when derivable.
    #[serde(rename = "deferred_resolved")]
    DeferredResolved {
        tool_use_id: String,
        agent_id: String,
        tool_name: String,
        ts: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer_summary: Option<String>,
    },

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
    AgentList {
        agents: Vec<ManagedAgent>,
        /// Echoed correlation ID from the request. Broadcast agent lists omit
        /// this, so one-shot CLI callers can distinguish their direct reply.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        correlation_id: Option<String>,
    },

    /// Correlated direct acknowledgement that a managed-agent spawn was queued.
    /// The legacy `NewDecision` broadcast remains unchanged for TUI clients.
    #[serde(rename = "agent_spawn_queued")]
    AgentSpawnQueued {
        decision: DecisionRequest,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        correlation_id: Option<String>,
    },

    /// Correlated direct response to StopAgent. The legacy `AgentExited`
    /// broadcast remains unchanged for TUI clients.
    #[serde(rename = "agent_stop_response")]
    AgentStopResponse {
        agent_id: String,
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        correlation_id: Option<String>,
    },

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

    /// Response to [`ClientMessage::QueryWorktrees`]: read-only working-tree
    /// status per active project (itr#401, spec §5.3).
    #[serde(rename = "worktrees_response")]
    WorktreesResponse { worktrees: Vec<WorktreeStatus> },

    /// Response to [`ClientMessage::QueryBurn`]: recent approved
    /// artifact-candidate tool calls from the decision log (itr#402,
    /// spec §5.4 burn meter).
    #[serde(rename = "burn_response")]
    BurnResponse { touches: Vec<ArtifactTouch> },

    /// Response to [`ClientMessage::QueryProjectHookStatus`]: the project's
    /// current hook install state (itr#460).
    #[serde(rename = "project_hook_status")]
    ProjectHookStatus(ProjectHookStatus),

    /// Result of an [`ClientMessage::InstallHooks`] command (itr#460). On
    /// success `status` carries the freshly-audited hook state and `error` is
    /// None; on failure `status` is None and `error` carries the message.
    #[serde(rename = "install_hooks_result")]
    InstallHooksResult {
        project: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        status: Option<ProjectHookStatus>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error: Option<String>,
    },

    /// Full snapshot of currently registered agents, sent to TUI on connect.
    #[serde(rename = "agents_snapshot")]
    AgentsSnapshot { agents: Vec<AgentInfo> },

    /// Response to ReimportEvents: how many events were imported.
    #[serde(rename = "reimport_complete")]
    ReimportComplete { count: u64 },

    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },

    // ── Web UI auth events ────────────────────────────────────────────
    /// A web login attempt failed. Broadcast to TUI clients so humans can
    /// see suspicious activity on the host. `attempt_count` is the running
    /// tally for this `ip` within the throttle window.
    #[serde(rename = "web_login_failure")]
    WebLoginFailure {
        ip: String,
        attempt_count: u32,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// A web login attempt succeeded; a device token was issued.
    #[serde(rename = "web_login_success")]
    WebLoginSuccess {
        device_id: String,
        device_name: String,
        ip: String,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// A new passkey was enrolled against an existing device.
    #[serde(rename = "web_device_registered")]
    WebDeviceRegistered {
        device_id: String,
        device_name: String,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// A web device was revoked (logged out or explicitly revoked from TUI/CLI).
    #[serde(rename = "web_device_revoked")]
    WebDeviceRevoked {
        device_id: String,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// A sudo-class approve from `device_id` was rejected because the device's
    /// reauth grace window has expired. The web frontend uses this to open
    /// the sudo modal.
    ///
    /// `request_id` carries the id of the specific DecisionRequest that got
    /// gated — the browser needs it to correlate the reauth with the exact
    /// approve to retry. Without it, rapid back-to-back sudo approves race:
    /// the browser's single-slot stash would be clobbered by the later event
    /// and the earlier request would block until the daemon's decision
    /// timeout fires.
    #[serde(rename = "web_reauth_required")]
    WebReauthRequired {
        device_id: String,
        request_id: String,
        tool_name: String,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// Daemon acknowledges it has processed a [`ClientMessage::MarkDeviceFresh`]
    /// for `device_id`. The HTTP `/api/auth/reauth` handler waits for this
    /// before returning 200 to the browser — otherwise a racing follow-up
    /// approve could arrive at the daemon before the registry update has
    /// landed and still trip the sudo gate.
    #[serde(rename = "mark_device_fresh_ack")]
    MarkDeviceFreshAck { device_id: String },

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

    /// A non-destructive resource alert. Wisphive never auto-deletes audit data
    /// (see itr#340); when the audit archive grows large or the host is low on
    /// disk it raises this instead, surfaced as a TUI/web banner. `active=false`
    /// is a clear: the condition dropped back below its threshold.
    #[serde(rename = "disk_alert")]
    DiskAlert {
        kind: DiskAlertKind,
        active: bool,
        message: String,
        at: chrono::DateTime<chrono::Utc>,
    },

    /// A non-blocking configuration trust or policy-widening alert. The
    /// daemon keeps accepting work with safe defaults when config is
    /// untrusted; this gives the operator immediate evidence in TUI/web.
    #[serde(rename = "config_alert")]
    ConfigAlert {
        kind: ConfigAlertKind,
        active: bool,
        message: String,
        at: chrono::DateTime<chrono::Utc>,
    },
}

/// Which resource condition a [`ServerMessage::DiskAlert`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskAlertKind {
    /// The on-disk audit archive has grown past its alert threshold.
    ArchiveSize,
    /// Free space on the Wisphive state filesystem dropped below the floor.
    LowDiskSpace,
}

/// Which configuration condition a [`ServerMessage::ConfigAlert`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAlertKind {
    /// The effective approval policy became more permissive.
    PolicyWidened,
    /// The user config cannot be trusted as a policy input.
    UntrustedConfig,
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
        let msg = ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
        };
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
                assert_eq!(
                    updated_input.unwrap(),
                    serde_json::json!({"command": "cargo test"})
                );
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
        let msg = ServerMessage::Error {
            message: "something went wrong".into(),
        };
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
        let msg = ServerMessage::DecisionResolved {
            id,
            decision: Decision::Deny,
        };
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
            decided_by: Some("level:all".into()),
            config_hash: None,
        };
        let msg = ServerMessage::HistoryResponse {
            entries: vec![entry],
            request_id: Some("req-123".into()),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::HistoryResponse {
                entries,
                request_id,
            } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].tool_name, "Bash");
                assert_eq!(request_id.unwrap(), "req-123");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_audit_messages() {
        let audit = AuditDecision {
            kind: crate::types::AuditDecisionKind::AutoApproved,
            decided_by: Some("level:all".into()),
            project: PathBuf::from("/proj"),
            agent_id: "cc-1".into(),
            terminal_session_id: Some(uuid::Uuid::new_v4()),
            tool_name: "Read".into(),
            ts: chrono::Utc::now(),
            tool_use_id: None,
            resolved: None,
            tool_input: None,
        };

        let msg = ServerMessage::AuditDecision(audit.clone());
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::AuditDecision(decoded) => assert_eq!(decoded, audit),
            _ => panic!("unexpected variant"),
        }

        let snapshot = ServerMessage::AuditSnapshot {
            items: vec![audit.clone()],
        };
        let encoded = encode(&snapshot).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::AuditSnapshot { items } => assert_eq!(items, vec![audit]),
            _ => panic!("unexpected variant"),
        }

        // A deferred decision carries the redacted tool_input so the inbox can
        // render the literal question/options; assert it survives round-trip.
        let deferred = AuditDecision {
            kind: crate::types::AuditDecisionKind::Deferred,
            decided_by: Some("always_ask:intrinsic".into()),
            project: PathBuf::from("/proj"),
            agent_id: "cc-2".into(),
            terminal_session_id: None,
            tool_name: "AskUserQuestion".into(),
            ts: chrono::Utc::now(),
            tool_use_id: Some("toolu_abc123".into()),
            resolved: None,
            tool_input: Some(serde_json::json!({
                "questions": [
                    { "question": "Ship it?", "options": [{ "label": "Yes" }] }
                ]
            })),
        };
        let encoded = encode(&ServerMessage::AuditDecision(deferred.clone())).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::AuditDecision(decoded) => {
                assert_eq!(decoded, deferred);
                assert_eq!(
                    decoded.tool_input.as_ref().unwrap()["questions"][0]["question"],
                    serde_json::json!("Ship it?")
                );
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
            confirm: true,
            filter: Some(DecisionFilter {
                tool_name: Some("Bash".into()),
                project: Some(PathBuf::from("/proj")),
                agent_type: Some(AgentType::ClaudeCode),
            }),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::ApproveAll { filter, confirm } => {
                assert!(confirm);
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
            ClientMessage::QueryHistory {
                agent_id,
                limit,
                request_id,
            } => {
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
            ..Default::default()
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
    fn round_trip_query_worktrees() {
        let msg = ClientMessage::QueryWorktrees;
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"query_worktrees\""));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ClientMessage::QueryWorktrees));
    }

    #[test]
    fn round_trip_worktrees_response() {
        use crate::types::{WorktreeChange, WorktreeStatus};
        let wt = WorktreeStatus {
            project: PathBuf::from("/Users/test/proj"),
            is_git_repo: true,
            branch: Some("main".into()),
            detached: false,
            head: Some("abc123def4567890abc123def4567890abc123de".into()),
            upstream: Some("origin/main".into()),
            ahead: Some(2),
            behind: Some(0),
            changes: vec![
                WorktreeChange {
                    path: "src/lib.rs".into(),
                    status: ".M".into(),
                    orig_path: None,
                    attributed_to: Some("cc-worker-1".into()),
                    attributed_tool: Some("Edit".into()),
                },
                WorktreeChange {
                    path: "notes.txt".into(),
                    status: "??".into(),
                    orig_path: None,
                    attributed_to: None,
                    attributed_tool: None,
                },
                WorktreeChange {
                    path: "b.txt".into(),
                    status: "R.".into(),
                    orig_path: Some("a.txt".into()),
                    attributed_to: None,
                    attributed_tool: None,
                },
            ],
            changes_truncated: false,
            diffstat: Some("1 file changed, 3 insertions(+)".into()),
            probed_at: chrono::Utc::now(),
            error: None,
        };
        let msg = ServerMessage::WorktreesResponse {
            worktrees: vec![wt.clone()],
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"worktrees_response\""));
        // Elided-when-None fields must not appear for the untracked entry.
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::WorktreesResponse { worktrees } => {
                assert_eq!(worktrees, vec![wt]);
            }
            _ => panic!("unexpected variant"),
        }
    }

    /// A minimal frame from an older/leaner daemon (optional fields absent)
    /// must still decode — the additive-fields contract.
    #[test]
    fn decode_minimal_worktree_status_is_backward_compatible() {
        let minimal = r#"{"type":"worktrees_response","worktrees":[{"project":"/p","is_git_repo":false,"probed_at":"2026-07-15T00:00:00Z"}]}"#;
        let decoded: ServerMessage = decode(minimal).unwrap();
        match decoded {
            ServerMessage::WorktreesResponse { worktrees } => {
                assert_eq!(worktrees.len(), 1);
                let wt = &worktrees[0];
                assert!(!wt.is_git_repo);
                assert!(wt.branch.is_none());
                assert!(!wt.detached);
                assert!(wt.changes.is_empty());
                assert!(!wt.changes_truncated);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_query_burn() {
        let msg = ClientMessage::QueryBurn;
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"query_burn\""));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ClientMessage::QueryBurn));
    }

    #[test]
    fn round_trip_burn_response() {
        use crate::types::ArtifactTouch;
        let touch = ArtifactTouch {
            agent_id: "cc-burn-1".into(),
            project: PathBuf::from("/proj/alpha"),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "git commit -m 'feat: x'"}),
            ts: chrono::Utc::now(),
        };
        let msg = ServerMessage::BurnResponse {
            touches: vec![touch.clone()],
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"burn_response\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::BurnResponse { touches } => {
                assert_eq!(touches, vec![touch]);
            }
            _ => panic!("unexpected variant"),
        }
    }

    /// A minimal frame from an older/leaner daemon (`tool_input` elided) must
    /// still decode — the additive-fields contract (mirrors the worktrees
    /// backward-compat test).
    #[test]
    fn decode_minimal_artifact_touch_is_backward_compatible() {
        let minimal = r#"{"type":"burn_response","touches":[{"agent_id":"cc-1","project":"/p","tool_name":"Write","ts":"2026-07-15T00:00:00Z"}]}"#;
        let decoded: ServerMessage = decode(minimal).unwrap();
        match decoded {
            ServerMessage::BurnResponse { touches } => {
                assert_eq!(touches.len(), 1);
                assert_eq!(touches[0].agent_id, "cc-1");
                assert_eq!(touches[0].tool_name, "Write");
                assert!(touches[0].tool_input.is_null());
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_install_hooks() {
        let msg = ClientMessage::InstallHooks {
            project: PathBuf::from("/Users/test/proj"),
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"install_hooks\""));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::InstallHooks { project } => {
                assert_eq!(project, PathBuf::from("/Users/test/proj"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_query_project_hook_status() {
        let msg = ClientMessage::QueryProjectHookStatus {
            project: PathBuf::from("/Users/test/proj"),
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"query_project_hook_status\""));
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::QueryProjectHookStatus { project } => {
                assert_eq!(project, PathBuf::from("/Users/test/proj"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    fn sample_hook_status() -> crate::types::ProjectHookStatus {
        crate::types::ProjectHookStatus {
            project: PathBuf::from("/Users/test/proj"),
            mode: "active".into(),
            claude_installed: true,
            codex_installed: false,
            missing_events: vec!["PreToolUse".into(), "Stop".into()],
            all_installed: false,
            all_enabled: false,
        }
    }

    #[test]
    fn round_trip_project_hook_status() {
        let status = sample_hook_status();
        let msg = ServerMessage::ProjectHookStatus(status.clone());
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"project_hook_status\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::ProjectHookStatus(s) => assert_eq!(s, status),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_install_hooks_result_success() {
        let status = sample_hook_status();
        let msg = ServerMessage::InstallHooksResult {
            project: PathBuf::from("/Users/test/proj"),
            status: Some(status.clone()),
            error: None,
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"install_hooks_result\""));
        assert!(!encoded.contains("\"error\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::InstallHooksResult {
                project,
                status: st,
                error,
            } => {
                assert_eq!(project, PathBuf::from("/Users/test/proj"));
                assert_eq!(st.unwrap(), status);
                assert!(error.is_none());
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_install_hooks_result_error() {
        let msg = ServerMessage::InstallHooksResult {
            project: PathBuf::from("/Users/test/proj"),
            status: None,
            error: Some("permission denied".into()),
        };
        let encoded = encode(&msg).unwrap();
        assert!(!encoded.contains("\"status\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::InstallHooksResult { status, error, .. } => {
                assert!(status.is_none());
                assert_eq!(error.unwrap(), "permission denied");
            }
            _ => panic!("unexpected variant"),
        }
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
            ClientMessage::AgentRegister {
                agent_id,
                agent_type,
                project,
            } => {
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
            group_name: None,
            sort_order: 0,
            created_by: None,
            replay_acl: Vec::new(),
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
            ClientMessage::TermCreate {
                label, cols, rows, ..
            } => {
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
        let msg = ClientMessage::TermInput {
            id,
            data: encoded_payload.clone(),
        };
        let line = encode(&msg).unwrap();
        let decoded: ClientMessage = decode(&line).unwrap();
        match decoded {
            ClientMessage::TermInput { id: did, data } => {
                assert_eq!(did, id);
                let round_tripped = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap();
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
            ClientMessage::TermClose { id },
            ClientMessage::TermList,
            ClientMessage::TermReplay {
                id,
                from_seq: Some(42),
                speed: Some(2.0),
            },
            ClientMessage::TermResize {
                id,
                cols: 100,
                rows: 30,
            },
        ] {
            let encoded = encode(&msg).unwrap();
            let _: ClientMessage = decode(&encoded).unwrap();
        }

        let encoded = encode(&ClientMessage::TermClose { id }).unwrap();
        assert!(!encoded.contains("kill"));

        // Older clients may still send the retired field. Serde ignores it,
        // preserving wire compatibility while the daemon exposes one behavior.
        let legacy = format!(r#"{{"type":"term_close","id":"{id}","kill":true}}"#);
        assert!(matches!(
            decode::<ClientMessage>(&legacy).unwrap(),
            ClientMessage::TermClose { id: decoded } if decoded == id
        ));
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
        let list = ServerMessage::TermListResponse {
            sessions: vec![meta],
        };
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
        assert_eq!(
            encoded.matches('\n').count(),
            1,
            "encoding must contain exactly one newline"
        );
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::TermChunk {
                direction, data: d, ..
            } => {
                assert_eq!(direction, TerminalDirection::Output);
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&d)
                    .unwrap();
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

        let done = ServerMessage::TermReplayDone {
            id,
            total_events: 42,
        };
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

    // ── Web UI auth events ──────────────────────────────────────────

    #[test]
    fn round_trip_web_login_failure() {
        let at = chrono::Utc::now();
        let msg = ServerMessage::WebLoginFailure {
            ip: "192.168.1.42".into(),
            attempt_count: 3,
            at,
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"web_login_failure\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::WebLoginFailure {
                ip, attempt_count, ..
            } => {
                assert_eq!(ip, "192.168.1.42");
                assert_eq!(attempt_count, 3);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_web_login_success() {
        let msg = ServerMessage::WebLoginSuccess {
            device_id: "dev-1".into(),
            device_name: "phone".into(),
            ip: "10.0.0.5".into(),
            at: chrono::Utc::now(),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        assert!(matches!(decoded, ServerMessage::WebLoginSuccess { .. }));
    }

    #[test]
    fn round_trip_web_device_registered_and_revoked() {
        let reg = ServerMessage::WebDeviceRegistered {
            device_id: "dev-2".into(),
            device_name: "laptop".into(),
            at: chrono::Utc::now(),
        };
        let _: ServerMessage = decode(&encode(&reg).unwrap()).unwrap();

        let rev = ServerMessage::WebDeviceRevoked {
            device_id: "dev-2".into(),
            at: chrono::Utc::now(),
        };
        let encoded = encode(&rev).unwrap();
        assert!(encoded.contains("\"type\":\"web_device_revoked\""));
        let _: ServerMessage = decode(&encoded).unwrap();
    }

    #[test]
    fn round_trip_web_reauth_required() {
        let msg = ServerMessage::WebReauthRequired {
            device_id: "dev-3".into(),
            request_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            tool_name: "Bash".into(),
            at: chrono::Utc::now(),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::WebReauthRequired {
                device_id,
                request_id,
                tool_name,
                ..
            } => {
                assert_eq!(device_id, "dev-3");
                assert_eq!(request_id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
                assert_eq!(tool_name, "Bash");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_mark_device_fresh_envelope() {
        // MarkDeviceFresh carries no body fields; attribution is on the envelope.
        let cmd = ClientCommand::from(ClientMessage::MarkDeviceFresh)
            .with_device_id(DeviceId::from("dev-4"));
        let encoded = encode(&cmd).unwrap();
        assert!(encoded.contains("\"type\":\"mark_device_fresh\""));
        assert!(encoded.contains("\"device_id\":\"dev-4\""));
        let decoded: ClientCommand = decode(&encoded).unwrap();
        assert!(matches!(decoded.body, ClientMessage::MarkDeviceFresh));
        assert_eq!(
            decoded.device_id.as_ref().map(|d| d.0.as_str()),
            Some("dev-4")
        );
    }

    #[test]
    fn round_trip_mark_device_fresh_ack() {
        let msg = ServerMessage::MarkDeviceFreshAck {
            device_id: "dev-5".into(),
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"mark_device_fresh_ack\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::MarkDeviceFreshAck { device_id } => assert_eq!(device_id, "dev-5"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn round_trip_config_alert() {
        let msg = ServerMessage::ConfigAlert {
            kind: ConfigAlertKind::PolicyWidened,
            active: true,
            message: "auto_approve_level increased from read to all".into(),
            at: chrono::Utc::now(),
        };
        let encoded = encode(&msg).unwrap();
        assert!(encoded.contains("\"type\":\"config_alert\""));
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::ConfigAlert {
                kind,
                active,
                message,
                ..
            } => {
                assert_eq!(kind, ConfigAlertKind::PolicyWidened);
                assert!(active);
                assert_eq!(message, "auto_approve_level increased from read to all");
            }
            _ => panic!("unexpected variant"),
        }
    }

    /// Legacy wire format (pre-envelope) embedded `device_id` on the variant
    /// itself. The [`ClientCommand`] envelope flattens the body, so those
    /// historical payloads still decode — the field lands on the envelope
    /// instead of the variant, and the envelope's `device_id` is populated.
    #[test]
    fn decode_legacy_approve_with_inline_device_id_is_backward_compatible() {
        let id = uuid::Uuid::new_v4();
        let legacy = format!(
            r#"{{"type":"approve","id":"{id}","always_allow":false,"device_id":"dev-abc"}}"#
        );
        let decoded: ClientCommand = decode(&legacy).unwrap();
        match decoded.body {
            ClientMessage::Approve { id: did, .. } => assert_eq!(did, id),
            _ => panic!("unexpected variant"),
        }
        assert_eq!(
            decoded.device_id.as_ref().map(|d| d.0.as_str()),
            Some("dev-abc")
        );
    }

    /// Payloads that pre-date both the envelope and the per-variant device_id
    /// (i.e. bare approves from a legacy TUI) must still decode with
    /// `device_id = None`.
    #[test]
    fn decode_legacy_approve_without_device_id_is_backward_compatible() {
        let id = uuid::Uuid::new_v4();
        let legacy = format!(r#"{{"type":"approve","id":"{id}","always_allow":false}}"#);
        let decoded: ClientCommand = decode(&legacy).unwrap();
        match decoded.body {
            ClientMessage::Approve { id: did, .. } => assert_eq!(did, id),
            _ => panic!("unexpected variant"),
        }
        assert!(decoded.device_id.is_none());
        assert!(decoded.correlation_id.is_none());
    }

    #[test]
    fn decode_uncorrelated_agent_list_is_backward_compatible() {
        let legacy = r#"{"type":"agent_list","agents":[]}"#;
        let decoded: ServerMessage = decode(legacy).unwrap();
        assert!(matches!(
            decoded,
            ServerMessage::AgentList {
                agents,
                correlation_id: None,
            } if agents.is_empty()
        ));
    }

    /// The envelope must elide `device_id` and `correlation_id` when None so encoding a
    /// plain [`ClientMessage`] stays byte-equivalent to a
    /// `ClientCommand { body, device_id: None }` — critical for existing
    /// callers (TUI, tests) that keep emitting bare ClientMessages.
    #[test]
    fn envelope_device_id_omitted_when_none() {
        let id = uuid::Uuid::new_v4();
        let command = ClientCommand::new(ClientMessage::Deny { id, message: None });
        let encoded = encode(&command).unwrap();
        assert!(
            !encoded.contains("device_id"),
            "wire output should omit device_id when None: {encoded}"
        );
        assert!(
            !encoded.contains("correlation_id"),
            "wire output should omit correlation_id when None: {encoded}"
        );

        // And the bare ClientMessage encoding must match the envelope's.
        let bare = encode(&ClientMessage::Deny { id, message: None }).unwrap();
        assert_eq!(
            encoded, bare,
            "envelope with None device_id must be byte-identical to bare ClientMessage"
        );
    }

    #[test]
    fn envelope_round_trip_with_device_id() {
        let id = uuid::Uuid::new_v4();
        let command = ClientCommand::new(ClientMessage::Approve {
            id,
            message: None,
            updated_input: None,
            always_allow: false,
            additional_context: None,
        })
        .with_device_id(DeviceId::from("dev-phone-7"));
        let encoded = encode(&command).unwrap();
        assert!(encoded.contains("\"device_id\":\"dev-phone-7\""));
        assert!(encoded.contains("\"type\":\"approve\""));
        let decoded: ClientCommand = decode(&encoded).unwrap();
        assert_eq!(decoded.device_id.unwrap().0, "dev-phone-7");
        match decoded.body {
            ClientMessage::Approve { id: did, .. } => assert_eq!(did, id),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn device_id_newtype_wire_format_is_bare_string() {
        let d = DeviceId::from("dev-42");
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(j, "\"dev-42\"");
        let back: DeviceId = serde_json::from_str(&j).unwrap();
        assert_eq!(back.0, "dev-42");
    }

    #[test]
    fn tag_based_discrimination() {
        let client_msg = ClientMessage::Hello {
            client: ClientType::Hook,
            version: PROTOCOL_VERSION,
        };
        let server_msg = ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
        };

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
