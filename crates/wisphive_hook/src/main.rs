use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use wisphive_protocol::{
    AgentType, ClientMessage, ClientType, Decision, DecisionRequest, HookEventType,
    PROTOCOL_VERSION, ServerMessage, ToolResult,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;

/// Tools that are always safe to auto-approve (read-only + orchestration).
/// Fallback when no config.json exists. Matches the Read tier.
const DEFAULT_AUTO_APPROVE: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "LSP",
    "NotebookRead",
    "WebSearch",
    "WebFetch",
    "Agent",
    "Skill",
    "ToolSearch",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "EnterWorktree",
    "ExitWorktree",
    "TaskCreate",
    "TaskUpdate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TodoRead",
    "CronList",
];

/// Hook response to format for the calling agent.
struct HookResponse {
    decision: Decision,
    message: Option<String>,
    updated_input: Option<serde_json::Value>,
    additional_context: Option<String>,
    /// For PermissionRequest: the selected suggestion to echo back.
    selected_permission: Option<wisphive_protocol::PermissionSuggestion>,
    /// The hook event type — determines the response JSON format.
    event_type: HookEventType,
    /// Agent implementation that invoked this hook.
    agent_type: AgentType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailMode {
    Open,
    Closed,
}

impl FailMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookFailureKind {
    Runtime,
    InputTooLarge,
    /// The daemon socket could not be reached (refused / absent / connect-level
    /// IO error). This is a control-plane outage, not a per-call failure: a
    /// crashed daemon must never brick every agent, so it always fails open
    /// regardless of `fail-mode`. Runtime parse/protocol errors stay fail-closed.
    DaemonUnreachable,
}

#[derive(Debug, Clone)]
struct HookFailure {
    kind: HookFailureKind,
    message: String,
    event_type: HookEventType,
    agent_type: AgentType,
}

impl HookFailure {
    fn before_parse(message: impl Into<String>) -> Self {
        Self {
            kind: HookFailureKind::Runtime,
            message: message.into(),
            event_type: HookEventType::PreToolUse,
            agent_type: detect_agent_type(&serde_json::Value::Null),
        }
    }

    fn input_too_large(max_bytes: usize) -> Self {
        Self {
            kind: HookFailureKind::InputTooLarge,
            message: format!(
                "Wisphive denied this hook because stdin exceeded the {} limit.",
                format_byte_limit(max_bytes)
            ),
            event_type: HookEventType::PreToolUse,
            agent_type: detect_agent_type(&serde_json::Value::Null),
        }
    }

    fn with_context(
        context: impl AsRef<str>,
        error: impl fmt::Display,
        event_type: HookEventType,
        agent_type: &AgentType,
    ) -> Self {
        Self::message(
            format!("{}: {error}", context.as_ref()),
            event_type,
            agent_type,
        )
    }

    fn message(
        message: impl Into<String>,
        event_type: HookEventType,
        agent_type: &AgentType,
    ) -> Self {
        Self {
            kind: HookFailureKind::Runtime,
            message: message.into(),
            event_type,
            agent_type: agent_type.clone(),
        }
    }

    /// Connect-level failure reaching the daemon socket. Tagged
    /// [`HookFailureKind::DaemonUnreachable`] so it always fails open.
    fn unreachable(
        context: impl AsRef<str>,
        error: impl fmt::Display,
        event_type: HookEventType,
        agent_type: &AgentType,
    ) -> Self {
        Self {
            kind: HookFailureKind::DaemonUnreachable,
            message: format!("{}: {error}", context.as_ref()),
            event_type,
            agent_type: agent_type.clone(),
        }
    }

    fn deny_response(&self) -> HookResponse {
        let mut response =
            HookResponse::new(Decision::Deny, self.event_type, self.agent_type.clone());
        response.message = Some(self.message.clone());
        response
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadInputError {
    TooLarge { max_bytes: usize },
    Io(String),
    InvalidUtf8(String),
}

impl fmt::Display for ReadInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { max_bytes } => write!(
                f,
                "hook input exceeded the {} limit",
                format_byte_limit(*max_bytes)
            ),
            Self::Io(message) => write!(f, "{message}"),
            Self::InvalidUtf8(message) => write!(f, "{message}"),
        }
    }
}

fn format_byte_limit(max_bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if max_bytes >= MIB && max_bytes.is_multiple_of(MIB) {
        format!("{} MiB", max_bytes / MIB)
    } else {
        format!("{max_bytes} bytes")
    }
}

impl HookResponse {
    fn simple(decision: Decision) -> Self {
        Self::new(decision, HookEventType::PreToolUse, AgentType::ClaudeCode)
    }

    fn new(decision: Decision, event_type: HookEventType, agent_type: AgentType) -> Self {
        Self {
            decision,
            message: None,
            updated_input: None,
            additional_context: None,
            selected_permission: None,
            event_type,
            agent_type,
        }
    }
}

fn detect_agent_type(hook_event: &serde_json::Value) -> AgentType {
    let env_value = std::env::var("WISPHIVE_AGENT_TYPE").ok();
    detect_agent_type_from_env(env_value.as_deref(), hook_event)
}

fn detect_agent_type_from_env(
    env_value: Option<&str>,
    hook_event: &serde_json::Value,
) -> AgentType {
    if let Some(value) = env_value {
        match value {
            "codex" => return AgentType::Codex,
            "claude_code" | "claude" => return AgentType::ClaudeCode,
            _ => {}
        }
    }

    if hook_event.get("model").is_some() || hook_event.get("turn_id").is_some() {
        return AgentType::Codex;
    }

    AgentType::ClaudeCode
}

fn agent_id_prefix(agent_type: &AgentType) -> &'static str {
    match agent_type {
        AgentType::Codex => "codex",
        AgentType::ClaudeCode => "cc",
        AgentType::Red => "red",
        AgentType::LocalLlm => "local",
    }
}

fn agent_project_env(agent_type: &AgentType) -> Option<&'static str> {
    match agent_type {
        AgentType::Codex => Some("CODEX_PROJECT_DIR"),
        AgentType::ClaudeCode => Some("CLAUDE_PROJECT_DIR"),
        AgentType::Red | AgentType::LocalLlm => None,
    }
}

fn main() {
    let code = format_and_exit(&run());
    process::exit(code);
}

fn mode_is_active(contents: Option<&str>) -> bool {
    contents.is_some_and(|mode| mode.trim() == "active")
}

fn is_active(wisphive_dir: &Path) -> bool {
    let mode_path = wisphive_dir.join("mode");
    mode_is_active(std::fs::read_to_string(&mode_path).ok().as_deref())
}

fn fail_mode_from_contents(contents: Option<&str>) -> FailMode {
    contents
        .and_then(FailMode::parse)
        .unwrap_or(FailMode::Closed)
}

fn read_fail_mode(wisphive_dir: &Path) -> FailMode {
    let fail_mode_path = wisphive_dir.join("fail-mode");
    fail_mode_from_contents(std::fs::read_to_string(&fail_mode_path).ok().as_deref())
}

fn response_for_failure(failure: &HookFailure, fail_mode: FailMode) -> HookResponse {
    let approve = || {
        HookResponse::new(
            Decision::Approve,
            failure.event_type,
            failure.agent_type.clone(),
        )
    };

    // PostToolUse is telemetry only — a reporting failure must never block a
    // tool call that already ran.
    if failure.event_type == HookEventType::PostToolUse {
        return approve();
    }

    // Oversized input always denies (DoS guard), regardless of fail-mode.
    if failure.kind == HookFailureKind::InputTooLarge {
        return failure.deny_response();
    }

    // A daemon outage always fails open: with the control plane down there is
    // no path to a human decision, and fail-closing here would brick every
    // agent on the machine. Honors the "daemon-down fails open" posture.
    if failure.kind == HookFailureKind::DaemonUnreachable {
        return approve();
    }

    // Other runtime failures (parse/protocol/IO) honor `fail-mode`, which
    // defaults to closed (deny) per the security posture in AGENTS.md.
    if fail_mode == FailMode::Closed {
        failure.deny_response()
    } else {
        approve()
    }
}

fn read_limited_to_string<R: Read>(reader: R, max_bytes: usize) -> Result<String, ReadInputError> {
    let limit = max_bytes.saturating_add(1) as u64;
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut limited_reader = reader.take(limit);
    limited_reader
        .read_to_end(&mut bytes)
        .map_err(|err| ReadInputError::Io(err.to_string()))?;

    if bytes.len() > max_bytes {
        return Err(ReadInputError::TooLarge { max_bytes });
    }

    String::from_utf8(bytes).map_err(|err| ReadInputError::InvalidUtf8(err.to_string()))
}

/// Format the hook response as agent-specific JSON stdout and return exit code.
fn format_and_exit(resp: &HookResponse) -> i32 {
    use HookEventType::*;
    match resp.event_type {
        PreToolUse => {}
        PermissionRequest => return format_permission_response(resp),
        PostToolUse | PostToolUseFailure => return format_post_tool_use_response(resp),
        Stop | SubagentStop => return format_stop_response(resp),
        UserPromptSubmit | ConfigChange | PreCompact => return format_block_response(resp),
        Elicitation | ElicitationResult => return format_elicitation_response(resp),
        TeammateIdle => return format_teammate_idle_response(resp),
        TaskCompleted => return format_task_completed_response(resp),
        InstructionsLoaded | SubagentStart | StopFailure | WorktreeCreate | WorktreeRemove
        | PostCompact | SessionStart | SessionEnd | Notification => {
            return format_lifecycle_event_response(resp);
        }
        Unknown => return format_unknown_event_response(resp),
    }
    match resp.decision {
        Decision::Ask => {
            if resp.agent_type == AgentType::Codex {
                return 0;
            }
            // Defer to native prompt
            let json = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask"
                }
            });
            print!("{}", json);
            0
        }
        Decision::Deny => {
            if let Some(ref msg) = resp.message {
                // Deny with feedback via JSON (agent sees the reason)
                let json = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": msg
                    }
                });
                print!("{}", json);
                0 // exit 0 because JSON controls behavior
            } else {
                2 // simple deny, same as before
            }
        }
        Decision::Approve => {
            if resp.agent_type == AgentType::Codex {
                if resp.updated_input.is_some() {
                    let json = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": "Wisphive blocked this tool call because Codex hooks do not support updatedInput yet. Re-run with the edited input."
                        }
                    });
                    print!("{}", json);
                    return 0;
                }

                if let Some(ref ctx) = resp.additional_context {
                    let json = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "additionalContext": ctx
                        }
                    });
                    print!("{}", json);
                }
                return 0;
            }

            if let Some(json) = pre_tool_use_approve_value(resp) {
                print!("{}", json);
            }
            0
        }
    }
}

/// Build the Claude PreToolUse approve JSON: `allow` plus any `updatedInput` /
/// `additionalContext`. Returns `None` when there are no extras (bare allow →
/// no stdout). `additionalContext` is nested INSIDE `hookSpecificOutput`
/// (alongside `permissionDecision`/`updatedInput`); emitting it as a top-level
/// sibling would make Claude Code silently ignore it.
fn pre_tool_use_approve_value(resp: &HookResponse) -> Option<serde_json::Value> {
    if resp.updated_input.is_none() && resp.additional_context.is_none() {
        return None;
    }

    let mut hook_output = serde_json::Map::new();
    hook_output.insert(
        "hookEventName".into(),
        serde_json::Value::String("PreToolUse".into()),
    );
    hook_output.insert(
        "permissionDecision".into(),
        serde_json::Value::String("allow".into()),
    );
    if let Some(ref input) = resp.updated_input {
        hook_output.insert("updatedInput".into(), input.clone());
    }
    if let Some(ref ctx) = resp.additional_context {
        hook_output.insert(
            "additionalContext".into(),
            serde_json::Value::String(ctx.clone()),
        );
    }

    Some(serde_json::json!({
        "hookSpecificOutput": serde_json::Value::Object(hook_output)
    }))
}

/// Format telemetry-only hook events so they never fall through to PreToolUse.
fn format_lifecycle_event_response(_resp: &HookResponse) -> i32 {
    0
}

/// Format unknown hook events loudly without emitting a wrong event shape.
fn format_unknown_event_response(_resp: &HookResponse) -> i32 {
    eprintln!("Wisphive ignored an unknown hook event. Update wisphive_protocol::HookEventType.");
    0
}

/// Format PostToolUse/PostToolUseFailure: result reporting is telemetry only, so never block or emit
/// PreToolUse-shaped JSON for a completed tool call.
fn format_post_tool_use_response(_resp: &HookResponse) -> i32 {
    0
}

/// Format a PermissionRequest response for the calling agent.
fn format_permission_response(resp: &HookResponse) -> i32 {
    let Some(json) = permission_response_value(resp) else {
        return 0;
    };

    print!("{}", json);
    0
}

fn permission_response_value(resp: &HookResponse) -> Option<serde_json::Value> {
    let decision_obj = permission_decision_object(resp)?;

    Some(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": serde_json::Value::Object(decision_obj)
        }
    }))
}

fn permission_decision_object(
    resp: &HookResponse,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut decision_obj = serde_json::Map::new();

    match resp.decision {
        Decision::Approve => {
            decision_obj.insert("behavior".into(), serde_json::Value::String("allow".into()));
            if resp.agent_type != AgentType::Codex
                && let Some(ref perm) = resp.selected_permission
            {
                decision_obj.insert(
                    "updatedPermissions".into(),
                    serde_json::to_value(vec![perm]).unwrap_or(serde_json::json!([])),
                );
            }
            if resp.agent_type != AgentType::Codex
                && let Some(ref input) = resp.updated_input
            {
                decision_obj.insert("updatedInput".into(), input.clone());
            }
        }
        Decision::Deny => {
            decision_obj.insert("behavior".into(), serde_json::Value::String("deny".into()));
            if let Some(ref msg) = resp.message {
                decision_obj.insert("message".into(), serde_json::Value::String(msg.clone()));
            }
        }
        Decision::Ask => return None,
    }

    Some(decision_obj)
}

/// Format Stop/SubagentStop: approve = let stop (exit 0), deny = continue working.
fn format_stop_response(resp: &HookResponse) -> i32 {
    match resp.decision {
        Decision::Approve => {
            if resp.agent_type == AgentType::Codex {
                return 0;
            }
            let json = serde_json::json!({"decision": "approve"});
            print!("{}", json);
            0
        }
        Decision::Deny => {
            let reason = resp.message.as_deref().unwrap_or("continue working");
            let json = serde_json::json!({"decision": "block", "reason": reason});
            print!("{}", json);
            0
        }
        Decision::Ask => 0,
    }
}

/// Format UserPromptSubmit/ConfigChange: approve = allow, deny = block.
fn format_block_response(resp: &HookResponse) -> i32 {
    match resp.decision {
        Decision::Approve => {
            // Inject reviewer-supplied context for both Claude and Codex. The
            // `hookSpecificOutput.{hookEventName, additionalContext}` shape is
            // accepted by both agents; gating it on Codex dropped the context
            // on the Claude path.
            if let Some(json) = block_additional_context_value(resp) {
                print!("{}", json);
            }
            0
        }
        Decision::Deny => {
            if let Some(ref msg) = resp.message {
                let json = serde_json::json!({"decision": "block", "reason": msg});
                print!("{}", json);
                0
            } else {
                2 // exit 2 = block
            }
        }
        Decision::Ask => 0,
    }
}

/// Build the `additionalContext` injection JSON for block-style events
/// (UserPromptSubmit / ConfigChange / PreCompact) on approve. Returns `None`
/// when there is no context to inject (bare approve → no stdout).
fn block_additional_context_value(resp: &HookResponse) -> Option<serde_json::Value> {
    let ctx = resp.additional_context.as_ref()?;
    Some(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": resp.event_type.to_string(),
            "additionalContext": ctx
        }
    }))
}

/// Format Elicitation: approve = accept with content, deny = decline/cancel.
fn format_elicitation_response(resp: &HookResponse) -> i32 {
    let action = match resp.decision {
        Decision::Approve => "accept",
        Decision::Deny => {
            if resp.message.as_deref() == Some("cancel") {
                "cancel"
            } else {
                "decline"
            }
        }
        Decision::Ask => return 0,
    };

    let mut output = serde_json::Map::new();
    let mut hook_output = serde_json::Map::new();
    hook_output.insert(
        "hookEventName".into(),
        serde_json::json!(resp.event_type.to_string()),
    );
    hook_output.insert("action".into(), serde_json::json!(action));
    if action == "accept"
        && let Some(ref input) = resp.updated_input
    {
        hook_output.insert("content".into(), input.clone());
    }
    output.insert(
        "hookSpecificOutput".into(),
        serde_json::Value::Object(hook_output),
    );
    print!("{}", serde_json::Value::Object(output));
    0
}

/// Format TeammateIdle: deny = continue with feedback (exit 2 + stderr), approve = stop.
fn format_teammate_idle_response(resp: &HookResponse) -> i32 {
    match resp.decision {
        Decision::Deny => {
            // Exit 2 = teammate gets feedback and continues working
            if let Some(ref msg) = resp.message {
                eprint!("{}", msg);
            }
            2
        }
        Decision::Approve => {
            // Stop the teammate
            let json = serde_json::json!({"continue": false, "stopReason": resp.message.as_deref().unwrap_or("stopped by user")});
            print!("{}", json);
            0
        }
        Decision::Ask => 0,
    }
}

/// Format TaskCompleted: approve = accept, deny = reject (exit 2 + stderr feedback).
fn format_task_completed_response(resp: &HookResponse) -> i32 {
    match resp.decision {
        Decision::Approve => 0,
        Decision::Deny => {
            if let Some(ref msg) = resp.message {
                eprint!("{}", msg);
            }
            2
        }
        Decision::Ask => 0,
    }
}

fn run() -> HookResponse {
    let home = home_dir();
    let wisphive_dir = home.join(".wisphive");

    if !is_active(&wisphive_dir) {
        return HookResponse::simple(Decision::Approve);
    }

    let fail_mode = read_fail_mode(&wisphive_dir);
    match run_active(&wisphive_dir) {
        Ok(response) => response,
        Err(failure) => response_for_failure(&failure, fail_mode),
    }
}

fn run_active(wisphive_dir: &Path) -> Result<HookResponse, HookFailure> {
    // Layer 2: Read agent hook data from stdin
    let input = read_limited_to_string(std::io::stdin().lock(), MAX_STDIN_BYTES).map_err(
        |err| match err {
            ReadInputError::TooLarge { max_bytes } => HookFailure::input_too_large(max_bytes),
            other => HookFailure::before_parse(format!("failed to read hook input: {other}")),
        },
    )?;

    let hook_event: serde_json::Value = serde_json::from_str(&input)
        .map_err(|err| HookFailure::before_parse(format!("failed to parse hook input: {err}")))?;

    // Determine event type from hook_event_name (early — needed for dispatch)
    let event_type: HookEventType = hook_event
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("PreToolUse")
        .parse()
        .unwrap_or_default();

    let agent_type = detect_agent_type(&hook_event);

    // PostToolUse detection: fire-and-forget result to daemon
    if event_type == HookEventType::PostToolUse || hook_event.get("tool_response").is_some() {
        let _ = handle_post_tool_use(&hook_event, wisphive_dir, agent_type.clone());
        let response_event_type = if event_type == HookEventType::PostToolUse {
            event_type
        } else {
            HookEventType::PostToolUse
        };
        return Ok(HookResponse::new(
            Decision::Approve,
            response_event_type,
            agent_type,
        ));
    }

    let is_permission_request = event_type == HookEventType::PermissionRequest;

    // For events without a tool_name (Stop, ConfigChange, etc.), use the event type name
    let tool_name = hook_event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| event_type.to_string());

    // Extract agent identity early (needed for registration before auto-approve check)
    let agent_id = hook_event
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| format!("{}-{}", agent_id_prefix(&agent_type), s))
        .or_else(|| std::env::var("WISPHIVE_AGENT_ID").ok())
        .unwrap_or_else(|| format!("{}-{}", agent_id_prefix(&agent_type), process::id()));

    let project = agent_project_env(&agent_type)
        .and_then(|key| std::env::var(key).ok())
        .map(PathBuf::from)
        .or_else(|| {
            hook_event
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let tool_input = hook_event
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tool_use_id = hook_event
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Layer 3: Register agent with daemon (once per session, fire-and-forget)
    register_agent_once(&agent_id, agent_type.clone(), &project, wisphive_dir);

    // Auto-approve certain event types based on config (with sensible defaults)
    if is_event_auto_approved(event_type, wisphive_dir) {
        // For events with null tool_input, log event_data instead so the context is preserved
        let log_input = if tool_input.is_null() {
            extract_event_data(event_type, &hook_event).unwrap_or(tool_input.clone())
        } else {
            tool_input.clone()
        };
        log_auto_approved(
            wisphive_dir,
            AutoApprovedLog {
                tool_use_id: &tool_use_id,
                agent_id: &agent_id,
                project: &project,
                tool_name: &tool_name,
                tool_input: &log_input,
                event_type,
                agent_type: &agent_type,
            },
        );
        return Ok(HookResponse::new(Decision::Approve, event_type, agent_type));
    }

    // Layer 4: Auto-approve check — PermissionRequests always go to daemon
    if !is_permission_request && is_auto_approved(&tool_name, &tool_input, wisphive_dir) {
        log_auto_approved(
            wisphive_dir,
            AutoApprovedLog {
                tool_use_id: &tool_use_id,
                agent_id: &agent_id,
                project: &project,
                tool_name: &tool_name,
                tool_input: &tool_input,
                event_type,
                agent_type: &agent_type,
            },
        );
        return Ok(HookResponse::new(Decision::Approve, event_type, agent_type));
    }

    // Parse permission suggestions for PermissionRequest events
    let permission_suggestions = if is_permission_request {
        hook_event.get("permission_suggestions").and_then(|v| {
            serde_json::from_value::<Vec<wisphive_protocol::PermissionSuggestion>>(v.clone()).ok()
        })
    } else {
        None
    };

    // Extract event-specific data for non-PreToolUse events
    let mut event_data = extract_event_data(event_type, &hook_event);

    // For ExitPlanMode, extract plan content from transcript
    if tool_name == "ExitPlanMode"
        && let Some(plan) = hook_event
            .get("transcript_path")
            .and_then(|v| v.as_str())
            .and_then(extract_plan_from_transcript)
    {
        let data =
            event_data.get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(obj) = data.as_object_mut() {
            obj.insert("plan_content".into(), serde_json::Value::String(plan));
        }
    }

    // Correlation with wisphive-managed terminal sessions: the daemon
    // exports WISPHIVE_TERMINAL_SESSION_ID into the PTY, which flows through
    // the shell to any Claude/Codex process and on into this hook.
    let terminal_session_id = std::env::var("WISPHIVE_TERMINAL_SESSION_ID")
        .ok()
        .and_then(|s| uuid::Uuid::parse_str(&s).ok());

    let request = DecisionRequest {
        id: uuid::Uuid::new_v4(),
        agent_id,
        agent_type: agent_type.clone(),
        project,
        tool_name,
        tool_input,
        timestamp: chrono::Utc::now(),
        hook_event_name: event_type,
        tool_use_id: tool_use_id.clone(),
        permission_suggestions,
        event_data,
        terminal_session_id,
    };

    // Layer 4: Connect to daemon socket (fails instantly if daemon is dead)
    let socket_path = wisphive_dir.join("wisphive.sock");
    let stream = UnixStream::connect(&socket_path).map_err(|err| {
        // A refused/absent socket means the daemon is down (e.g. crashed and
        // left a stale socket). Fail open so the outage can't brick agents.
        HookFailure::unreachable(
            "failed to connect to Wisphive daemon",
            err,
            event_type,
            &agent_type,
        )
    })?;
    // Socket-configuration failures on a just-connected stream mean the peer
    // went away between connect and setup → daemon down → fail open.
    stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .map_err(|err| {
            HookFailure::unreachable(
                "failed to configure daemon read timeout",
                err,
                event_type,
                &agent_type,
            )
        })?;
    stream
        .set_write_timeout(Some(SOCKET_TIMEOUT))
        .map_err(|err| {
            HookFailure::unreachable(
                "failed to configure daemon write timeout",
                err,
                event_type,
                &agent_type,
            )
        })?;

    let mut reader = BufReader::new(stream.try_clone().map_err(|err| {
        HookFailure::unreachable(
            "failed to clone daemon socket",
            err,
            event_type,
            &agent_type,
        )
    })?);
    let mut writer = stream;

    // Handshake. Encoding our own Hello is a local concern (keep Runtime), but
    // every transport step below — and a peer-closed/empty/garbled welcome —
    // means we never established a working session with a live daemon, so it
    // fails open. Only AFTER a valid Welcome do we treat the daemon as "up and
    // answering", where a refusal/garbage response is honored fail-closed.
    let hello = wisphive_protocol::encode(&ClientMessage::Hello {
        client: ClientType::Hook,
        version: PROTOCOL_VERSION,
    })
    .map_err(|err| {
        HookFailure::with_context(
            "failed to encode daemon hello",
            err,
            event_type,
            &agent_type,
        )
    })?;
    writer.write_all(hello.as_bytes()).map_err(|err| {
        HookFailure::unreachable("failed to write daemon hello", err, event_type, &agent_type)
    })?;

    let mut welcome_line = String::new();
    let welcome_bytes = reader.read_line(&mut welcome_line).map_err(|err| {
        HookFailure::unreachable(
            "failed to read daemon welcome",
            err,
            event_type,
            &agent_type,
        )
    })?;
    if welcome_bytes == 0 || welcome_line.trim().is_empty() {
        // EOF: the daemon closed the connection during the handshake (it
        // crashed or is shutting down). read_line returns Ok(0), not an error.
        return Err(HookFailure::unreachable(
            "Wisphive daemon closed the connection during handshake",
            "no welcome received (EOF)",
            event_type,
            &agent_type,
        ));
    }
    let welcome: ServerMessage = wisphive_protocol::decode(&welcome_line).map_err(|err| {
        HookFailure::unreachable(
            "failed to parse daemon welcome",
            err,
            event_type,
            &agent_type,
        )
    })?;
    if !matches!(welcome, ServerMessage::Welcome { .. }) {
        return Err(HookFailure::unreachable(
            "Wisphive daemon sent an unexpected welcome response",
            "handshake did not complete",
            event_type,
            &agent_type,
        ));
    }

    // Send decision request
    let req_msg =
        wisphive_protocol::encode(&ClientMessage::DecisionRequest(request)).map_err(|err| {
            HookFailure::with_context(
                "failed to encode decision request",
                err,
                event_type,
                &agent_type,
            )
        })?;
    writer.write_all(req_msg.as_bytes()).map_err(|err| {
        // Broken pipe writing the request = the daemon died → fail open.
        HookFailure::unreachable(
            "failed to write decision request",
            err,
            event_type,
            &agent_type,
        )
    })?;

    // Block for response — daemon controls timeout (up to 1 hour).
    writer.set_read_timeout(None).map_err(|err| {
        HookFailure::unreachable(
            "failed to clear daemon read timeout",
            err,
            event_type,
            &agent_type,
        )
    })?;

    let mut response_line = String::new();
    let response_bytes = reader.read_line(&mut response_line).map_err(|err| {
        HookFailure::unreachable(
            "failed to read daemon decision response",
            err,
            event_type,
            &agent_type,
        )
    })?;
    if response_bytes == 0 || response_line.trim().is_empty() {
        // EOF while blocked waiting for the human decision: the daemon was
        // killed mid-wait (e.g. crashed under load). This is the most common
        // real-world brick — fail open rather than deny.
        return Err(HookFailure::unreachable(
            "Wisphive daemon closed the connection before returning a decision",
            "no decision received (EOF)",
            event_type,
            &agent_type,
        ));
    }

    // A non-empty but unparseable response means a live daemon sent garbage —
    // treat as a reachable-but-misbehaving daemon and honor fail-mode (deny by
    // default), rather than silently approving.
    let response: ServerMessage = wisphive_protocol::decode(&response_line).map_err(|err| {
        HookFailure::with_context(
            "failed to parse daemon decision response",
            err,
            event_type,
            &agent_type,
        )
    })?;

    match response {
        ServerMessage::DecisionResponse {
            decision,
            message,
            updated_input,
            additional_context,
            selected_permission,
            ..
        } => Ok(HookResponse {
            decision,
            message,
            updated_input,
            additional_context,
            selected_permission,
            event_type,
            agent_type,
        }),
        ServerMessage::Error { message } => Err(HookFailure::message(
            format!("Wisphive daemon returned an error instead of a decision: {message}"),
            event_type,
            &agent_type,
        )),
        _ => Err(HookFailure::message(
            "Wisphive daemon sent an unexpected response to a decision request",
            event_type,
            &agent_type,
        )),
    }
}

/// Handle a PostToolUse event: fire-and-forget the result to the daemon.
fn handle_post_tool_use(
    hook_event: &serde_json::Value,
    wisphive_dir: &std::path::Path,
    agent_type: AgentType,
) -> Result<(), Box<dyn std::error::Error>> {
    let tool_name = hook_event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let tool_input = hook_event
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tool_result = hook_event
        .get("tool_response")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let agent_id = hook_event
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| format!("{}-{}", agent_id_prefix(&agent_type), s))
        .or_else(|| std::env::var("WISPHIVE_AGENT_ID").ok())
        .unwrap_or_else(|| format!("{}-{}", agent_id_prefix(&agent_type), std::process::id()));

    let tool_use_id = hook_event
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let socket_path = wisphive_dir.join("wisphive.sock");
    let stream = UnixStream::connect(&socket_path)?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    // Handshake
    let hello = wisphive_protocol::encode(&ClientMessage::Hello {
        client: ClientType::Hook,
        version: PROTOCOL_VERSION,
    })?;
    writer.write_all(hello.as_bytes())?;

    // Consume welcome
    let mut welcome_line = String::new();
    reader.read_line(&mut welcome_line)?;

    // Send tool result (fire-and-forget)
    let msg = wisphive_protocol::encode(&ClientMessage::ToolResult(ToolResult {
        agent_id,
        tool_name,
        tool_input,
        tool_result,
        timestamp: chrono::Utc::now(),
        tool_use_id,
    }))?;
    writer.write_all(msg.as_bytes())?;

    Ok(())
}

/// Register this agent session with the daemon (fire-and-forget).
/// Uses a marker file to ensure registration only happens once per session.
fn register_agent_once(
    agent_id: &str,
    agent_type: AgentType,
    project: &std::path::Path,
    wisphive_dir: &std::path::Path,
) {
    // Fast path: check marker file (single stat syscall)
    let sessions_dir = wisphive_dir.join("sessions");
    let marker = sessions_dir.join(agent_id);
    if marker.exists() {
        return;
    }

    // Attempt registration — all errors are swallowed (fail-open)
    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        let socket_path = wisphive_dir.join("wisphive.sock");
        let stream = UnixStream::connect(&socket_path)?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;

        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;

        // Handshake
        let hello = wisphive_protocol::encode(&ClientMessage::Hello {
            client: ClientType::Hook,
            version: PROTOCOL_VERSION,
        })?;
        writer.write_all(hello.as_bytes())?;

        let mut welcome_line = String::new();
        reader.read_line(&mut welcome_line)?;

        // Send AgentRegister (fire-and-forget)
        let msg = wisphive_protocol::encode(&ClientMessage::AgentRegister {
            agent_id: agent_id.to_string(),
            agent_type: agent_type.clone(),
            project: project.to_path_buf(),
        })?;
        writer.write_all(msg.as_bytes())?;

        // Create marker file
        let _ = std::fs::create_dir_all(&sessions_dir);
        let _ = std::fs::write(&marker, "");

        Ok(())
    })();
}

/// Check if an event type should be auto-approved based on config.
///
/// Config keys in ~/.wisphive/config.json:
///   "auto_approve_stop": bool          (default: false)
///   "auto_approve_user_prompt": bool   (default: true)
///   "auto_approve_config_change": bool (default: true)
///   "auto_approve_lifecycle": bool     (default: true)
///
/// Set to false to send these events to the daemon for review (useful for debugging).
fn is_event_auto_approved(
    event_type: wisphive_protocol::HookEventType,
    wisphive_dir: &std::path::Path,
) -> bool {
    use wisphive_protocol::HookEventType;

    let (config_key, default) = match event_type {
        HookEventType::Stop | HookEventType::SubagentStop => ("auto_approve_stop", false),
        HookEventType::UserPromptSubmit => ("auto_approve_user_prompt", true),
        HookEventType::ConfigChange => ("auto_approve_config_change", true),
        HookEventType::InstructionsLoaded
        | HookEventType::PostToolUseFailure
        | HookEventType::SubagentStart
        | HookEventType::StopFailure
        | HookEventType::WorktreeRemove
        | HookEventType::PostCompact
        | HookEventType::SessionStart
        | HookEventType::SessionEnd
        | HookEventType::Notification => ("auto_approve_lifecycle", true),
        _ => return false,
    };

    let config_path = wisphive_dir.join("config.json");
    std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|config| config.get(config_key)?.as_bool())
        .unwrap_or(default)
}

/// Check if a tool is auto-approved using tiered levels + content-aware rules.
///
/// Priority: auto_approve_remove → auto_approve_add → level → legacy → defaults.
/// Then tool_rules override: deny_patterns block auto-approved tools,
/// allow_patterns approve non-approved tools. Patterns are case-insensitive
/// substrings matched against the tool input text.
fn is_auto_approved(
    tool_name: &str,
    tool_input: &serde_json::Value,
    wisphive_dir: &std::path::Path,
) -> bool {
    let config_path = wisphive_dir.join("config.json");

    let config: Option<serde_json::Value> = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok());

    // Determine base approval from level/add/remove
    let base_approved = if let Some(ref config) = config {
        // Check explicit removals first
        if let Some(arr) = config.get("auto_approve_remove").and_then(|v| v.as_array()) {
            if arr.iter().any(|v| v.as_str() == Some(tool_name)) {
                false
            } else {
                check_base_approved(config, tool_name, wisphive_dir)
            }
        } else {
            check_base_approved(config, tool_name, wisphive_dir)
        }
    } else {
        // No config.json — check legacy then defaults
        legacy_auto_approved(tool_name, wisphive_dir)
    };

    // Apply content-aware tool_rules
    if let Some(ref config) = config
        && let Some(rules) = config.get("tool_rules").and_then(|v| v.as_object())
        && let Some(rule) = rules.get(tool_name)
    {
        let input_text = tool_input_text(tool_name, tool_input);
        let input_lower = input_text.to_lowercase();

        if base_approved {
            // Check deny_patterns — any match blocks auto-approve
            if let Some(patterns) = rule.get("deny_patterns").and_then(|v| v.as_array()) {
                for p in patterns {
                    if let Some(pat) = p.as_str()
                        && input_lower.contains(&pat.to_lowercase())
                    {
                        return false;
                    }
                }
            }
        } else {
            // Check allow_patterns — any match auto-approves
            if let Some(patterns) = rule.get("allow_patterns").and_then(|v| v.as_array()) {
                for p in patterns {
                    if let Some(pat) = p.as_str()
                        && input_lower.contains(&pat.to_lowercase())
                    {
                        return true;
                    }
                }
            }
        }
    }

    base_approved
}

/// Check base approval from explicit additions and tiered level.
fn check_base_approved(
    config: &serde_json::Value,
    tool_name: &str,
    wisphive_dir: &std::path::Path,
) -> bool {
    // Check explicit additions
    if let Some(arr) = config.get("auto_approve_add").and_then(|v| v.as_array())
        && arr.iter().any(|v| v.as_str() == Some(tool_name))
    {
        return true;
    }

    // Check tiered level
    if let Some(level_str) = config.get("auto_approve_level").and_then(|v| v.as_str())
        && let Ok(level) = level_str.parse::<wisphive_protocol::AutoApproveLevel>()
    {
        return level.includes(tool_name);
    }

    // Fallback to legacy
    legacy_auto_approved(tool_name, wisphive_dir)
}

/// Check legacy auto-approve.json and built-in defaults.
fn legacy_auto_approved(tool_name: &str, wisphive_dir: &std::path::Path) -> bool {
    let legacy_path = wisphive_dir.join("auto-approve.json");
    if legacy_path.exists()
        && let Ok(content) = std::fs::read_to_string(&legacy_path)
        && let Ok(config) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(arr) = config.get("auto_approve").and_then(|v| v.as_array())
    {
        return arr.iter().any(|v| v.as_str() == Some(tool_name));
    }
    DEFAULT_AUTO_APPROVE.contains(&tool_name)
}

/// Log an auto-approved tool call to events.jsonl for daemon ingestion.
/// Uses O_APPEND for atomic writes (~0.1-1μs). All errors are swallowed (fail-open).
struct AutoApprovedLog<'a> {
    tool_use_id: &'a Option<String>,
    agent_id: &'a str,
    project: &'a std::path::Path,
    tool_name: &'a str,
    tool_input: &'a serde_json::Value,
    event_type: HookEventType,
    agent_type: &'a AgentType,
}

fn log_auto_approved(wisphive_dir: &std::path::Path, log: AutoApprovedLog<'_>) {
    let path = wisphive_dir.join("events.jsonl");
    let entry = serde_json::json!({
        "event": "auto_approved",
        "hook_event_name": log.event_type.to_string(),
        "tool_use_id": log.tool_use_id,
        "agent_id": log.agent_id,
        "agent_type": log.agent_type.to_string(),
        "project": log.project,
        "tool_name": log.tool_name,
        "tool_input": log.tool_input,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    let mut line = serde_json::to_string(&entry).unwrap_or_default();
    line.push('\n');
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// Extract the text to match patterns against for a given tool.
/// For Bash: the `command` field. For everything else: JSON-serialized input.
fn tool_input_text(tool_name: &str, tool_input: &serde_json::Value) -> String {
    if tool_name == "Bash"
        && let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str())
    {
        return cmd.to_string();
    }
    serde_json::to_string(tool_input).unwrap_or_default()
}

/// Extract event-specific data from the hook event payload.
fn extract_event_data(
    event_type: wisphive_protocol::HookEventType,
    hook_event: &serde_json::Value,
) -> Option<serde_json::Value> {
    use wisphive_protocol::HookEventType::*;
    match event_type {
        Elicitation | ElicitationResult => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("mcp_server_name") {
                data.insert("mcp_server_name".into(), v.clone());
            }
            if let Some(v) = hook_event.get("message") {
                data.insert("message".into(), v.clone());
            }
            if let Some(v) = hook_event.get("mode") {
                data.insert("mode".into(), v.clone());
            }
            if let Some(v) = hook_event.get("requested_schema") {
                data.insert("requested_schema".into(), v.clone());
            }
            if let Some(v) = hook_event.get("url") {
                data.insert("url".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        Stop | SubagentStop => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("last_assistant_message") {
                data.insert("last_assistant_message".into(), v.clone());
            }
            if let Some(v) = hook_event.get("stop_hook_active") {
                data.insert("stop_hook_active".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        UserPromptSubmit => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("prompt") {
                data.insert("prompt".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        ConfigChange => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("source") {
                data.insert("source".into(), v.clone());
            }
            if let Some(v) = hook_event.get("file_path") {
                data.insert("file_path".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        TeammateIdle => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("teammate_name") {
                data.insert("teammate_name".into(), v.clone());
            }
            if let Some(v) = hook_event.get("team_name") {
                data.insert("team_name".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        TaskCompleted => {
            let mut data = serde_json::Map::new();
            if let Some(v) = hook_event.get("task_id") {
                data.insert("task_id".into(), v.clone());
            }
            if let Some(v) = hook_event.get("task_subject") {
                data.insert("task_subject".into(), v.clone());
            }
            if let Some(v) = hook_event.get("task_description") {
                data.insert("task_description".into(), v.clone());
            }
            Some(serde_json::Value::Object(data))
        }
        _ => None,
    }
}

/// Read the transcript JSONL and extract the last assistant text content (the plan).
///
/// Reads the file backwards, looking for the most recent assistant message
/// that contains text content. Returns the concatenated text blocks.
fn extract_plan_from_transcript(path: &str) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    // Collect all lines, then iterate backwards to find the last assistant text.
    // For typical transcripts this is fast enough; the file is small.
    let lines: Vec<String> = reader.lines().map_while(|l| l.ok()).collect();

    for line in lines.iter().rev() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Only look at assistant messages
        if entry.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        let content = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())?;

        // Collect all text blocks from this message
        let mut text_parts = Vec::new();
        for item in content {
            if item.get("type").and_then(|v| v.as_str()) == Some("text")
                && let Some(text) = item.get("text").and_then(|v| v.as_str())
            {
                text_parts.push(text.to_string());
            }
        }

        if !text_parts.is_empty() {
            return Some(text_parts.join("\n"));
        }
    }

    None
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fail_mode_parses_valid_values() {
        assert_eq!(fail_mode_from_contents(Some("open\n")), FailMode::Open);
        assert_eq!(fail_mode_from_contents(Some("closed")), FailMode::Closed);
    }

    #[test]
    fn fail_mode_defaults_to_closed_when_missing_or_invalid() {
        assert_eq!(fail_mode_from_contents(None), FailMode::Closed);
        assert_eq!(fail_mode_from_contents(Some("invalid")), FailMode::Closed);
        assert_eq!(fail_mode_from_contents(Some("")), FailMode::Closed);
    }

    #[test]
    fn missing_or_inactive_mode_is_not_active() {
        assert!(!mode_is_active(None));
        assert!(!mode_is_active(Some("off")));
        assert!(mode_is_active(Some("active\n")));
    }

    #[test]
    fn closed_fail_mode_turns_runtime_failure_into_deny() {
        let failure = HookFailure::message(
            "failed to connect to Wisphive daemon",
            HookEventType::PermissionRequest,
            &AgentType::Codex,
        );

        let response = response_for_failure(&failure, FailMode::Closed);

        assert_eq!(response.decision, Decision::Deny);
        assert_eq!(response.event_type, HookEventType::PermissionRequest);
        assert_eq!(response.agent_type, AgentType::Codex);
        let decision = permission_decision_object(&response).unwrap();
        assert_eq!(decision.get("behavior"), Some(&json!("deny")));
        assert_eq!(
            decision.get("message"),
            Some(&json!("failed to connect to Wisphive daemon"))
        );
    }

    #[test]
    fn post_tool_use_failures_do_not_block_when_fail_mode_is_closed() {
        let failure = HookFailure::message(
            "failed to send PostToolUse result to Wisphive daemon",
            HookEventType::PostToolUse,
            &AgentType::Codex,
        );

        let response = response_for_failure(&failure, FailMode::Closed);

        assert_eq!(response.decision, Decision::Approve);
        assert_eq!(response.event_type, HookEventType::PostToolUse);
        assert_eq!(response.agent_type, AgentType::Codex);
    }

    #[test]
    fn post_tool_use_formatter_never_blocks() {
        let mut response =
            HookResponse::new(Decision::Deny, HookEventType::PostToolUse, AgentType::Codex);
        response.message = Some("reporting failed".into());

        assert_eq!(format_post_tool_use_response(&response), 0);
    }

    #[test]
    fn open_fail_mode_preserves_runtime_failure_approval() {
        let failure = HookFailure::message(
            "failed to parse daemon response",
            HookEventType::PermissionRequest,
            &AgentType::Codex,
        );

        let response = response_for_failure(&failure, FailMode::Open);

        assert_eq!(response.decision, Decision::Approve);
        // Fail-open preserves the originating event type so the formatter emits
        // the correct shape (here a PermissionRequest allow, not a PreToolUse).
        assert_eq!(response.event_type, HookEventType::PermissionRequest);
        assert_eq!(response.agent_type, AgentType::Codex);
    }

    #[test]
    fn closed_fail_mode_denies_runtime_failure() {
        // A parse/protocol error (the daemon answered but we couldn't use it)
        // honors fail-closed and denies.
        let failure = HookFailure::message(
            "failed to parse daemon response",
            HookEventType::PermissionRequest,
            &AgentType::ClaudeCode,
        );

        let response = response_for_failure(&failure, FailMode::Closed);

        assert_eq!(response.decision, Decision::Deny);
        assert_eq!(response.event_type, HookEventType::PermissionRequest);
    }

    #[test]
    fn daemon_unreachable_fails_open_even_when_fail_mode_is_closed() {
        // Regression for the stale-socket brick: a crashed daemon (refused or
        // absent socket) must never block agents, even under fail-closed.
        for event_type in [
            HookEventType::PreToolUse,
            HookEventType::PermissionRequest,
            HookEventType::Stop,
        ] {
            let failure = HookFailure::unreachable(
                "failed to connect to Wisphive daemon",
                "Connection refused (os error 61)",
                event_type,
                &AgentType::ClaudeCode,
            );

            let response = response_for_failure(&failure, FailMode::Closed);

            assert_eq!(
                response.decision,
                Decision::Approve,
                "daemon-unreachable must fail open for {event_type:?}"
            );
            assert_eq!(response.event_type, event_type);
        }
    }

    #[test]
    fn oversized_input_denies_even_when_fail_mode_is_open() {
        let failure = HookFailure::input_too_large(MAX_STDIN_BYTES);

        let response = response_for_failure(&failure, FailMode::Open);

        assert_eq!(response.decision, Decision::Deny);
        assert_eq!(response.event_type, HookEventType::PreToolUse);
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("8 MiB")
        );
    }

    #[test]
    fn limited_stdin_accepts_exact_limit() {
        let input = "abcd";

        let output = read_limited_to_string(Cursor::new(input), 4).unwrap();

        assert_eq!(output, input);
    }

    #[test]
    fn limited_stdin_rejects_over_limit() {
        let err = read_limited_to_string(Cursor::new("abcde"), 4).unwrap_err();

        assert_eq!(err, ReadInputError::TooLarge { max_bytes: 4 });
    }

    use serde_json::json;

    #[test]
    fn detects_codex_from_env_override() {
        let event = json!({"hook_event_name": "PreToolUse"});
        assert_eq!(
            detect_agent_type_from_env(Some("codex"), &event),
            AgentType::Codex
        );
    }

    #[test]
    fn detects_claude_from_env_override() {
        let event = json!({"hook_event_name": "PreToolUse", "model": "gpt-5.4"});
        assert_eq!(
            detect_agent_type_from_env(Some("claude_code"), &event),
            AgentType::ClaudeCode
        );
    }

    #[test]
    fn detects_codex_from_codex_fields() {
        let event = json!({"hook_event_name": "PreToolUse", "turn_id": "turn-1"});
        assert_eq!(detect_agent_type_from_env(None, &event), AgentType::Codex);
    }

    #[test]
    fn defaults_to_claude_without_codex_signal() {
        let event = json!({"hook_event_name": "PreToolUse"});
        assert_eq!(
            detect_agent_type_from_env(None, &event),
            AgentType::ClaudeCode
        );
    }

    #[test]
    fn codex_permission_response_omits_reserved_fields() {
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::PermissionRequest,
            AgentType::Codex,
        );
        resp.updated_input = Some(json!({"command": "echo safe"}));
        resp.selected_permission = Some(wisphive_protocol::PermissionSuggestion {
            suggestion_type: "addRules".into(),
            rules: vec![],
            behavior: "allow".into(),
            destination: "session".into(),
            mode: None,
        });

        let decision = permission_decision_object(&resp).unwrap();
        assert_eq!(decision.get("behavior"), Some(&json!("allow")));
        assert!(!decision.contains_key("updatedInput"));
        assert!(!decision.contains_key("updatedPermissions"));
    }

    #[test]
    fn claude_permission_response_matches_documented_envelope() {
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::PermissionRequest,
            AgentType::ClaudeCode,
        );
        resp.updated_input = Some(json!({"command": "npm run lint"}));
        resp.selected_permission = Some(wisphive_protocol::PermissionSuggestion {
            suggestion_type: "addRules".into(),
            rules: vec![wisphive_protocol::PermissionRule {
                tool_name: "Bash".into(),
                rule_content: "npm run lint".into(),
            }],
            behavior: "allow".into(),
            destination: "session".into(),
            mode: None,
        });

        let output = permission_response_value(&resp).unwrap();

        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow",
                        "updatedInput": {
                            "command": "npm run lint"
                        },
                        "updatedPermissions": [{
                            "type": "addRules",
                            "rules": [{
                                "toolName": "Bash",
                                "ruleContent": "npm run lint"
                            }],
                            "behavior": "allow",
                            "destination": "session"
                        }]
                    }
                }
            })
        );
    }

    #[test]
    fn codex_permission_response_matches_documented_envelope_without_reserved_fields() {
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::PermissionRequest,
            AgentType::Codex,
        );
        resp.updated_input = Some(json!({"command": "npm run lint"}));
        resp.selected_permission = Some(wisphive_protocol::PermissionSuggestion {
            suggestion_type: "addRules".into(),
            rules: vec![wisphive_protocol::PermissionRule {
                tool_name: "Bash".into(),
                rule_content: "npm run lint".into(),
            }],
            behavior: "allow".into(),
            destination: "session".into(),
            mode: None,
        });

        let output = permission_response_value(&resp).unwrap();

        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow"
                    }
                }
            })
        );
    }

    #[test]
    fn permission_deny_response_matches_documented_envelope() {
        let mut resp = HookResponse::new(
            Decision::Deny,
            HookEventType::PermissionRequest,
            AgentType::Codex,
        );
        resp.message = Some("Blocked by repository policy.".into());

        let output = permission_response_value(&resp).unwrap();

        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "deny",
                        "message": "Blocked by repository policy."
                    }
                }
            })
        );
    }

    #[test]
    fn claude_pre_tool_use_approve_nests_additional_context_in_hook_specific_output() {
        // Regression for itr#356: additionalContext must live INSIDE
        // hookSpecificOutput (sibling of permissionDecision/updatedInput), not
        // at the top level where Claude Code silently ignores it.
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::PreToolUse,
            AgentType::ClaudeCode,
        );
        resp.updated_input = Some(json!({"command": "echo safe"}));
        resp.additional_context = Some("guidance for Claude".into());

        let output = pre_tool_use_approve_value(&resp).unwrap();

        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "updatedInput": {"command": "echo safe"},
                    "additionalContext": "guidance for Claude"
                }
            })
        );
        // The bug was a top-level sibling; assert it is NOT there.
        assert!(output.get("additionalContext").is_none());
    }

    #[test]
    fn pre_tool_use_approve_without_extras_emits_nothing() {
        let resp = HookResponse::new(
            Decision::Approve,
            HookEventType::PreToolUse,
            AgentType::ClaudeCode,
        );
        assert!(pre_tool_use_approve_value(&resp).is_none());
    }

    #[test]
    fn claude_block_approve_emits_additional_context() {
        // Regression for itr#357: a Claude UserPromptSubmit approve carrying
        // reviewer context must emit it, not drop it (was Codex-only).
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::UserPromptSubmit,
            AgentType::ClaudeCode,
        );
        resp.additional_context = Some("remember to run the lints".into());

        let output = block_additional_context_value(&resp).unwrap();

        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": "remember to run the lints"
                }
            })
        );
    }

    #[test]
    fn codex_block_approve_still_emits_additional_context() {
        // The fix removed the Codex-only gate; Codex must keep emitting too.
        let mut resp = HookResponse::new(
            Decision::Approve,
            HookEventType::ConfigChange,
            AgentType::Codex,
        );
        resp.additional_context = Some("config reloaded".into());

        let output = block_additional_context_value(&resp).unwrap();

        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            json!("ConfigChange")
        );
        assert_eq!(
            output["hookSpecificOutput"]["additionalContext"],
            json!("config reloaded")
        );
    }

    #[test]
    fn block_approve_without_context_emits_nothing() {
        let resp = HookResponse::new(
            Decision::Approve,
            HookEventType::UserPromptSubmit,
            AgentType::ClaudeCode,
        );
        assert!(block_additional_context_value(&resp).is_none());
    }
}
