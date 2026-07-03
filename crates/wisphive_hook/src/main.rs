use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use wisphive_protocol::{
    AgentType, ClientMessage, ClientType, DEFAULT_ALWAYS_ASK, Decision, DecisionRequest,
    HookEventType, PROTOCOL_VERSION, ServerMessage, ToolResult,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;

/// Maximum bytes a single newline-delimited response line from the daemon may
/// occupy before the hook rejects it (itr#83). Without a cap, a misbehaving or
/// hostile daemon peer that streams bytes with no newline would grow the hook's
/// read buffer until OOM. Aligned with `MAX_STDIN_BYTES` (8 MiB) — comfortably
/// above any legitimate welcome/decision response.
const MAX_LINE_BYTES: usize = MAX_STDIN_BYTES;

/// Read one newline-delimited line from a blocking reader, capping it at
/// `max_bytes` (itr#83). Mirrors [`std::io::BufRead::read_line`] semantics
/// (returns the number of bytes read; `0` = EOF; `line` keeps the trailing
/// `\n`) but bounds memory: it appends a byte at a time and bails with an error
/// the moment the line would exceed the cap, so a peer that never sends a
/// newline can't grow `line` past the cap and OOM the hook.
fn read_line_capped<R: BufRead>(
    reader: &mut R,
    line: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            break; // EOF
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(idx) => {
                if buf.len() + idx + 1 > max_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("daemon line exceeded {max_bytes}-byte cap"),
                    ));
                }
                buf.extend_from_slice(&available[..=idx]); // include the '\n'
                let consumed = idx + 1;
                reader.consume(consumed);
                break;
            }
            None => {
                let take = available.len();
                if buf.len() + take > max_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("daemon line exceeded {max_bytes}-byte cap"),
                    ));
                }
                buf.extend_from_slice(available);
                reader.consume(take);
            }
        }
    }
    let text = String::from_utf8(buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let n = text.len();
    line.push_str(&text);
    Ok(n)
}

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

    // Oversized input always denies (DoS guard), regardless of fail-mode AND
    // regardless of event type — checked before the PostToolUse telemetry
    // early-return so the documented "oversized stdin always denies"
    // guarantee is absolute (itr#344). For PostToolUse the deny is inert
    // anyway (the formatter exits 0 for telemetry), so nothing already-ran
    // gets blocked.
    if failure.kind == HookFailureKind::InputTooLarge {
        return failure.deny_response();
    }

    // PostToolUse is telemetry only — a reporting failure must never block a
    // tool call that already ran.
    if failure.event_type == HookEventType::PostToolUse {
        return approve();
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
    let (json, exit_code) = pre_tool_use_stdout(resp);
    if let Some(json) = json {
        print!("{}", json);
    }
    exit_code
}

/// Build the PreToolUse stdout JSON (if any) and exit code for a decision.
/// Pure so the agent-specific decision mapping is unit-testable.
fn pre_tool_use_stdout(resp: &HookResponse) -> (Option<serde_json::Value>, i32) {
    match resp.decision {
        Decision::Ask => {
            if resp.agent_type == AgentType::Codex {
                // Codex has no native PreToolUse prompt to defer to (it uses
                // PermissionRequest for native approvals), so "ask" cannot be
                // expressed — and exit 0 with empty stdout would be a silent
                // approve. Fail closed with a reason instead (itr#366).
                let json = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": "Wisphive cannot defer to a native prompt on Codex; re-run after explicit approval in the Wisphive TUI/web UI."
                    }
                });
                return (Some(json), 0);
            }
            // Defer to native prompt
            let json = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask"
                }
            });
            (Some(json), 0)
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
                (Some(json), 0) // exit 0 because JSON controls behavior
            } else {
                (None, 2) // simple deny, same as before
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
                    return (Some(json), 0);
                }

                if let Some(ref ctx) = resp.additional_context {
                    let json = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "additionalContext": ctx
                        }
                    });
                    return (Some(json), 0);
                }
                return (None, 0);
            }

            (pre_tool_use_approve_value(resp), 0)
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

/// Classify a payload as PostToolUse telemetry (auto-approved, result
/// forwarded fire-and-forget).
///
/// `hook_event_name` is authoritative (itr#346): a payload that EXPLICITLY
/// declares another event type is never reclassified by shape — a PreToolUse
/// carrying a smuggled `tool_response` field must still be gated, not waved
/// through as telemetry. The shape check only applies when no event name was
/// declared at all (Codex-style result reports omit it).
fn is_post_tool_use(event_type: HookEventType, hook_event: &serde_json::Value) -> bool {
    if event_type == HookEventType::PostToolUse {
        return true;
    }
    hook_event.get("hook_event_name").is_none() && hook_event.get("tool_response").is_some()
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
    if is_post_tool_use(event_type, &hook_event) {
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

    // Always-defer classification: questions, plan-mode, elicitations, and any
    // operator-designated harmful tools carry a human answer back only through
    // the agent's native prompt. Auto-approving them silently drops the answer
    // ("did not answer"), so they ALWAYS defer to the native prompt — even at
    // auto_approve_level=all. Intrinsic interactive prompts defer even under the
    // "dangerous" posture (see is_always_deferred); only operator-added harmful
    // tools are released by it. Evaluated before the auto-approve layers so the
    // level can't override it.
    //
    // This fires for BOTH PreToolUse and PermissionRequest. PermissionRequest is
    // the ONLY path that carries the human's answer back for these tools, so it
    // must defer too: Decision::Ask emits no decision object (see
    // permission_decision_object), letting Claude's native dialog render the
    // question/plan and capture the selection. Routing it to the daemon instead
    // (the old `!is_permission_request` guard) auto-resolved the prompt with no
    // selection — "Allowed by PermissionRequest hook" → "did not answer". See
    // itr#388 (regression on the itr#380 / ADR-0002 always-defer work).
    let defer_class = always_defer_classification(&tool_name, wisphive_dir);
    if matches!(defer_class, DeferClass::Intrinsic | DeferClass::Operator) {
        // Audit the deferral (itr#397): this decision was made by policy, not
        // a human, and used to leave no trace anywhere. On the Codex PreToolUse
        // path Ask cannot defer (itr#366) — the real effect is a fail-closed
        // deny, and the audit record says so.
        let base = match defer_class {
            DeferClass::Intrinsic => "always_ask:intrinsic",
            _ => "always_ask:operator",
        };
        let (event, decided_by) =
            if agent_type == AgentType::Codex && event_type == HookEventType::PreToolUse {
                ("denied", format!("codex_ask_fail_closed:{base}"))
            } else {
                ("deferred", base.to_string())
            };
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
                event,
                decided_by: &decided_by,
            },
        );
        return Ok(HookResponse::new(Decision::Ask, event_type, agent_type));
    }

    // Control-plane self-protection (itr#425, ADR-0005 I9): a Write/Edit/Bash
    // that targets ~/.wisphive/** must never ride an auto_approve_level — a gated
    // agent holding a generic write approval could otherwise rewrite its own gate
    // (config.json, mode, events.jsonl, the DB). When true, this skips BOTH
    // auto-approve layers below so the call falls through to daemon human review.
    // It deliberately does NOT return Decision::Ask: Ask hands the call to
    // Claude's native prompt, which `hooks install` has already allow-listed for
    // Write/Edit/Bash — that would auto-allow the self-edit and defeat the guard.
    // The `allow_self_modification: true` opt-in disables this (named-flag escape
    // hatch per the working agreement). When the daemon is unreachable the tiered
    // posture fails open (ADR-0001), but gating is then off for every tool anyway,
    // so the meaningful case — daemon up, level high — is the one this closes.
    let force_human_review = !allow_self_modification(wisphive_dir)
        && targets_control_plane(&tool_name, &tool_input, wisphive_dir);

    // Auto-approve certain event types based on config (with sensible defaults)
    if !force_human_review
        && let Some(toggle_key) = event_auto_approved_by(event_type, wisphive_dir)
    {
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
                event: "auto_approved",
                decided_by: &format!("event_toggle:{toggle_key}"),
            },
        );
        return Ok(HookResponse::new(Decision::Approve, event_type, agent_type));
    }

    // Layer 4: Auto-approve check — PermissionRequests always go to daemon
    if !force_human_review
        && !is_permission_request
        && let Some(rule) = auto_approved_by(&tool_name, &tool_input, wisphive_dir)
    {
        // An operator always_ask tool released by the dangerous posture is a
        // policy bypass the audit record must show explicitly (itr#397).
        let decided_by = if defer_class == DeferClass::ReleasedByDangerous {
            format!("auto_approve_dangerous:{rule}")
        } else {
            rule
        };
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
                event: "auto_approved",
                decided_by: &decided_by,
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

    // Kept for the itr#397 audit record written after the daemon responds —
    // the originals move into the request below.
    let audit_agent_id = agent_id.clone();
    let audit_project = project.clone();
    let audit_tool_name = tool_name.clone();
    let audit_tool_input = tool_input.clone();

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
    let welcome_bytes =
        read_line_capped(&mut reader, &mut welcome_line, MAX_LINE_BYTES).map_err(|err| {
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
    let response_bytes = read_line_capped(&mut reader, &mut response_line, MAX_LINE_BYTES)
        .map_err(|err| {
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
        } => {
            // A daemon-resolved Ask on the Codex PreToolUse path becomes a
            // fail-closed deny (itr#366) — and that non-human outcome must be
            // in the audit trail (itr#397). The daemon skips logging Ask
            // resolutions, so this record is the only trace.
            if decision == Decision::Ask
                && agent_type == AgentType::Codex
                && event_type == HookEventType::PreToolUse
            {
                log_auto_approved(
                    wisphive_dir,
                    AutoApprovedLog {
                        tool_use_id: &tool_use_id,
                        agent_id: &audit_agent_id,
                        project: &audit_project,
                        tool_name: &audit_tool_name,
                        tool_input: &audit_tool_input,
                        event_type,
                        agent_type: &agent_type,
                        event: "denied",
                        decided_by: "codex_ask_fail_closed:daemon_ask",
                    },
                );
            }
            Ok(HookResponse {
                decision,
                message,
                updated_input,
                additional_context,
                selected_permission,
                event_type,
                agent_type,
            })
        }
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

    // Consume welcome (capped so a hostile daemon can't OOM the hook — itr#83)
    let mut welcome_line = String::new();
    read_line_capped(&mut reader, &mut welcome_line, MAX_LINE_BYTES)?;

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

        // Consume welcome (capped — itr#83)
        let mut welcome_line = String::new();
        read_line_capped(&mut reader, &mut welcome_line, MAX_LINE_BYTES)?;

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
/// Returns the toggle key that approved the event (itr#397 audit), or `None`.
fn event_auto_approved_by(
    event_type: wisphive_protocol::HookEventType,
    wisphive_dir: &std::path::Path,
) -> Option<&'static str> {
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
        _ => return None,
    };

    let config_path = wisphive_dir.join("config.json");
    let enabled = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|config| config.get(config_key)?.as_bool())
        .unwrap_or(default);
    enabled.then_some(config_key)
}

/// Truncated SHA-256 of ~/.wisphive/config.json at decision time, for the
/// audit trail (itr#397): a policy weakening becomes correlatable with the
/// decisions it produced. `None` when the file is absent/unreadable.
fn config_snapshot_hash(wisphive_dir: &std::path::Path) -> Option<String> {
    use sha2::Digest;
    let bytes = std::fs::read(wisphive_dir.join("config.json")).ok()?;
    let digest = sha2::Sha256::digest(&bytes);
    // 16 hex chars (64 bits) is plenty to distinguish config revisions.
    Some(
        digest
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

/// How the always-defer classification resolved for a tool (itr#397 audit).
#[derive(Debug, PartialEq, Eq)]
enum DeferClass {
    /// Built-in interactive prompt (questions/plan-mode/elicitations) —
    /// defers unconditionally.
    Intrinsic,
    /// Operator-designated `always_ask` tool — defers under the balanced
    /// posture.
    Operator,
    /// Would have deferred (operator entry) but the `auto_approve_dangerous`
    /// posture released it — the auto-approve layers decide, and the audit
    /// record must show the bypass.
    ReleasedByDangerous,
    /// Not in the always-defer set.
    No,
}

/// Check whether a tool/event must always defer to the agent's native prompt.
///
/// Intrinsic interactive prompts — the built-in [`DEFAULT_ALWAYS_ASK`] set
/// (questions, plan-mode, elicitations) — defer UNCONDITIONALLY: their answer
/// travels back only through the agent's native prompt, so "auto-approving" one
/// approves nothing and silently discards the human's selection ("did not
/// answer"). Neither the `auto_approve_dangerous` posture nor an
/// `always_ask_remove` entry can make the daemon answer them.
///
/// Operator-designated harmful tools (config.json `always_ask`) also defer, but
/// they ARE released by the "dangerous" posture or an `always_ask_remove` entry.
fn always_defer_classification(tool_name: &str, wisphive_dir: &std::path::Path) -> DeferClass {
    // Intrinsic interactive prompts win over every config posture — evaluated
    // first so nothing below can un-defer a question/plan/elicitation. (The
    // "dangerous" posture used to re-swallow these, dropping the human's answer.)
    if DEFAULT_ALWAYS_ASK.contains(&tool_name) {
        return DeferClass::Intrinsic;
    }

    let Some(config) = std::fs::read_to_string(wisphive_dir.join("config.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
    else {
        return DeferClass::No;
    };

    let dangerous = config
        .get("auto_approve_dangerous")
        .and_then(|v| v.as_bool())
        == Some(true);

    // Operator removals win over operator additions.
    if let Some(arr) = config.get("always_ask_remove").and_then(|v| v.as_array())
        && arr.iter().any(|v| v.as_str() == Some(tool_name))
    {
        return DeferClass::No;
    }

    // Operator additions (e.g. harmful-action tools) — released only by the
    // dangerous posture, and that release is audit-visible.
    let operator_listed = config
        .get("always_ask")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(tool_name)));
    match (operator_listed, dangerous) {
        (true, true) => DeferClass::ReleasedByDangerous,
        (true, false) => DeferClass::Operator,
        (false, _) => DeferClass::No,
    }
}

/// Boolean view of [`always_defer_classification`].
#[cfg(test)]
fn is_always_deferred(tool_name: &str, wisphive_dir: &std::path::Path) -> bool {
    matches!(
        always_defer_classification(tool_name, wisphive_dir),
        DeferClass::Intrinsic | DeferClass::Operator
    )
}

/// Check if a tool is auto-approved using tiered levels + content-aware rules,
/// returning the rule that decided (itr#397 audit) or `None` when the tool
/// must queue for human review.
///
/// Priority: auto_approve_remove → auto_approve_add → level → legacy → defaults.
/// Then tool_rules override: deny_patterns block auto-approved tools,
/// allow_patterns approve non-approved tools. Patterns are case-insensitive
/// substrings matched against the tool input text.
fn auto_approved_by(
    tool_name: &str,
    tool_input: &serde_json::Value,
    wisphive_dir: &std::path::Path,
) -> Option<String> {
    let config_path = wisphive_dir.join("config.json");

    let config: Option<serde_json::Value> = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok());

    // Determine base approval from level/add/remove
    let base_rule = if let Some(ref config) = config {
        // Check explicit removals first
        let removed = config
            .get("auto_approve_remove")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(tool_name)));
        if removed {
            None
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

        if base_rule.is_some() {
            // Check deny_patterns — any match blocks auto-approve
            if let Some(patterns) = rule.get("deny_patterns").and_then(|v| v.as_array()) {
                for p in patterns {
                    if let Some(pat) = p.as_str()
                        && input_lower.contains(&pat.to_lowercase())
                    {
                        return None;
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
                        return Some(format!("tool_rules:{tool_name}:allow_pattern"));
                    }
                }
            }
        }
    }

    base_rule
}

/// Boolean view of [`auto_approved_by`].
#[cfg(test)]
fn is_auto_approved(
    tool_name: &str,
    tool_input: &serde_json::Value,
    wisphive_dir: &std::path::Path,
) -> bool {
    auto_approved_by(tool_name, tool_input, wisphive_dir).is_some()
}

/// Check base approval from explicit additions and tiered level, returning
/// the matching rule identifier.
fn check_base_approved(
    config: &serde_json::Value,
    tool_name: &str,
    wisphive_dir: &std::path::Path,
) -> Option<String> {
    // Check explicit additions
    if let Some(arr) = config.get("auto_approve_add").and_then(|v| v.as_array())
        && arr.iter().any(|v| v.as_str() == Some(tool_name))
    {
        return Some("auto_approve_add".to_string());
    }

    // Check tiered level
    if let Some(level_str) = config.get("auto_approve_level").and_then(|v| v.as_str())
        && let Ok(level) = level_str.parse::<wisphive_protocol::AutoApproveLevel>()
    {
        return level.includes(tool_name).then(|| format!("level:{level}"));
    }

    // Fallback to legacy
    legacy_auto_approved(tool_name, wisphive_dir)
}

/// Check legacy auto-approve.json and built-in defaults, returning the rule
/// identifier for a match.
fn legacy_auto_approved(tool_name: &str, wisphive_dir: &std::path::Path) -> Option<String> {
    let legacy_path = wisphive_dir.join("auto-approve.json");
    if legacy_path.exists()
        && let Ok(content) = std::fs::read_to_string(&legacy_path)
        && let Ok(config) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(arr) = config.get("auto_approve").and_then(|v| v.as_array())
    {
        return arr
            .iter()
            .any(|v| v.as_str() == Some(tool_name))
            .then(|| "legacy_list".to_string());
    }
    // Default fallback (no config.json, no legacy auto-approve.json): the Read
    // tier, sourced from wisphive_protocol so the list lives in exactly one place
    // (itr#121). Questions/plan-mode are intentionally absent — they go through
    // the always-defer classification, not auto-approval.
    wisphive_protocol::AutoApproveLevel::Read
        .tier_tools()
        .contains(&tool_name)
        .then(|| "default:read_tier".to_string())
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
    /// Record kind: "auto_approved" (default), "deferred", or "denied".
    event: &'a str,
    /// The layer/rule that made the decision (itr#397), e.g. "level:all",
    /// "always_ask:intrinsic", "event_toggle:auto_approve_stop".
    decided_by: &'a str,
}

fn log_auto_approved(wisphive_dir: &std::path::Path, log: AutoApprovedLog<'_>) {
    let path = wisphive_dir.join("events.jsonl");
    let entry = serde_json::json!({
        "event": if log.event.is_empty() { "auto_approved" } else { log.event },
        "hook_event_name": log.event_type.to_string(),
        "tool_use_id": log.tool_use_id,
        "agent_id": log.agent_id,
        "agent_type": log.agent_type.to_string(),
        "project": log.project,
        "tool_name": log.tool_name,
        // events.jsonl is durable (ingested into decision_log, archived) —
        // scrub credentials before they leave the hook (itr#89).
        "tool_input": wisphive_protocol::redact::redact_value(log.tool_input),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "decided_by": log.decided_by,
        "config_hash": config_snapshot_hash(wisphive_dir),
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

/// Operator opt-out for control-plane self-protection (itr#425). Absent/false
/// keeps the guard on — the secure default; `true` lets the agent edit its own
/// `~/.wisphive` state without human review.
fn allow_self_modification(wisphive_dir: &std::path::Path) -> bool {
    std::fs::read_to_string(wisphive_dir.join("config.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|cfg| cfg.get("allow_self_modification")?.as_bool())
        .unwrap_or(false)
}

/// Whether a tool call would touch the wisphive control-plane state dir
/// (itr#425, ADR-0005 I9). File tools (`Write`/`Edit`/`MultiEdit`/`NotebookEdit`)
/// match on a lexically-normalized path; `Bash` matches on a best-effort
/// substring of the canonical dir and its `~`/`$HOME` spellings — a backstop,
/// explicitly not soundness (a command can obfuscate the path arbitrarily).
fn targets_control_plane(
    tool_name: &str,
    tool_input: &serde_json::Value,
    wisphive_dir: &std::path::Path,
) -> bool {
    let path_field = match tool_name {
        "Write" | "Edit" | "MultiEdit" => Some("file_path"),
        "NotebookEdit" => Some("notebook_path"),
        "Bash" => {
            return tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|c| command_references_dir(c, wisphive_dir));
        }
        _ => None,
    };
    path_field
        .and_then(|f| tool_input.get(f))
        .and_then(|v| v.as_str())
        .is_some_and(|p| path_in_dir(p, wisphive_dir))
}

/// Best-effort test of whether a tool-supplied path resolves inside `dir`.
/// Expands a leading `~`/`$HOME`/`${HOME}` (relative to `dir`'s parent, i.e. the
/// home that owns the state dir), lexically normalizes (`.`/`..` collapsed —
/// without touching the filesystem, since the target may not exist yet), then
/// checks component-wise containment. Relative paths are left as-is: the agent's
/// cwd is the project, not the state dir, and the Bash substring backstop covers
/// spellings this misses.
fn path_in_dir(raw: &str, dir: &std::path::Path) -> bool {
    let home = dir.parent().unwrap_or(dir);
    let expanded = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if let Some(rest) = raw
        .strip_prefix("${HOME}/")
        .or_else(|| raw.strip_prefix("$HOME/"))
    {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    };
    lexical_normalize(&expanded).starts_with(dir)
}

/// Collapse `.` and `..` components lexically (no filesystem access).
fn lexical_normalize(p: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Best-effort substring backstop for Bash commands that name the state dir.
fn command_references_dir(cmd: &str, wisphive_dir: &std::path::Path) -> bool {
    let canonical = wisphive_dir.to_string_lossy();
    cmd.contains(canonical.as_ref())
        || ["~/.wisphive", "$HOME/.wisphive", "${HOME}/.wisphive"]
            .iter()
            .any(|spelling| cmd.contains(spelling))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Write a config.json into a fresh temp dir and return both.
    fn dir_with_config(config: serde_json::Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn questions_always_defer_with_no_config() {
        let dir = tempfile::tempdir().unwrap();
        for tool in [
            "AskUserQuestion",
            "EnterPlanMode",
            "ExitPlanMode",
            "Elicitation",
        ] {
            assert!(
                is_always_deferred(tool, dir.path()),
                "{tool} should always defer"
            );
        }
        assert!(!is_always_deferred("Bash", dir.path()));
    }

    #[test]
    fn questions_defer_even_at_level_all() {
        // The core fix: auto_approve_level=all must NOT auto-approve questions.
        let dir = dir_with_config(serde_json::json!({ "auto_approve_level": "all" }));
        assert!(is_always_deferred("AskUserQuestion", dir.path()));
        // ...and a normal tool is still auto-approved at level all (defer guard
        // only intercepts the always-ask set).
        assert!(!is_always_deferred("Bash", dir.path()));
        assert!(is_auto_approved(
            "Bash",
            &serde_json::Value::Null,
            dir.path()
        ));
    }

    #[test]
    fn oversized_input_denies_even_for_post_tool_use() {
        // itr#344: the "oversized stdin always denies" guarantee must be
        // absolute — the PostToolUse telemetry early-return cannot precede it.
        let failure = HookFailure {
            kind: HookFailureKind::InputTooLarge,
            message: "too large".into(),
            event_type: HookEventType::PostToolUse,
            agent_type: AgentType::ClaudeCode,
        };
        let resp = response_for_failure(&failure, FailMode::Open);
        assert_eq!(resp.decision, Decision::Deny);
    }

    #[test]
    fn explicit_pre_tool_use_with_smuggled_tool_response_is_not_telemetry() {
        // itr#346: hook_event_name is authoritative. A PreToolUse payload
        // carrying a tool_response field must still be gated, not
        // auto-approved as PostToolUse telemetry.
        let smuggled = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "rm -rf /"},
            "tool_response": {"output": "smuggled"},
        });
        assert!(!is_post_tool_use(HookEventType::PreToolUse, &smuggled));

        // Explicit PostToolUse stays telemetry.
        let post = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_response": {"output": "x"},
        });
        assert!(is_post_tool_use(HookEventType::PostToolUse, &post));

        // Codex-style result report with NO declared event name still counts.
        let codex = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": {"output": "x"},
        });
        assert!(is_post_tool_use(HookEventType::PreToolUse, &codex));
    }

    #[test]
    fn auto_approved_by_names_the_deciding_rule() {
        // itr#397: the audit trail must record WHICH layer approved.
        let dir = dir_with_config(serde_json::json!({
            "auto_approve_level": "read",
            "auto_approve_add": ["Bash"],
            "tool_rules": {"Edit": {"allow_patterns": ["/tmp/scratch"], "deny_patterns": []}},
        }));
        assert_eq!(
            auto_approved_by("Bash", &serde_json::Value::Null, dir.path()).as_deref(),
            Some("auto_approve_add")
        );
        assert_eq!(
            auto_approved_by("Read", &serde_json::Value::Null, dir.path()).as_deref(),
            Some("level:read")
        );
        assert_eq!(
            auto_approved_by(
                "Edit",
                &serde_json::json!({"file_path": "/tmp/scratch/x"}),
                dir.path()
            )
            .as_deref(),
            Some("tool_rules:Edit:allow_pattern")
        );
        // Not covered by anything → queued for a human, no rule.
        assert_eq!(
            auto_approved_by(
                "Edit",
                &serde_json::json!({"file_path": "/src/x"}),
                dir.path()
            ),
            None
        );
    }

    #[test]
    fn defer_classification_distinguishes_intrinsic_operator_and_dangerous_release() {
        let dir = dir_with_config(serde_json::json!({
            "auto_approve_level": "all",
            "always_ask": ["HarmfulTool"],
        }));
        assert_eq!(
            always_defer_classification("AskUserQuestion", dir.path()),
            DeferClass::Intrinsic
        );
        assert_eq!(
            always_defer_classification("HarmfulTool", dir.path()),
            DeferClass::Operator
        );
        assert_eq!(
            always_defer_classification("Bash", dir.path()),
            DeferClass::No
        );

        // Dangerous posture releases operator entries — visibly (itr#397) —
        // but never intrinsic prompts.
        let dir = dir_with_config(serde_json::json!({
            "auto_approve_level": "all",
            "auto_approve_dangerous": true,
            "always_ask": ["HarmfulTool"],
        }));
        assert_eq!(
            always_defer_classification("HarmfulTool", dir.path()),
            DeferClass::ReleasedByDangerous
        );
        assert_eq!(
            always_defer_classification("AskUserQuestion", dir.path()),
            DeferClass::Intrinsic
        );
    }

    #[test]
    fn config_snapshot_hash_tracks_config_content() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(config_snapshot_hash(dir.path()), None, "no config → None");

        std::fs::write(dir.path().join("config.json"), "{\"a\":1}").unwrap();
        let h1 = config_snapshot_hash(dir.path()).unwrap();
        assert_eq!(h1.len(), 16);

        std::fs::write(dir.path().join("config.json"), "{\"a\":2}").unwrap();
        let h2 = config_snapshot_hash(dir.path()).unwrap();
        assert_ne!(h1, h2, "different config content → different hash");
    }

    #[test]
    fn codex_ask_fails_closed_with_explicit_deny() {
        // itr#366: Codex has no native PreToolUse prompt, so Ask cannot defer —
        // and exit 0 with empty stdout would be a silent approve of a gated
        // tool. Ask must map to an explicit deny-with-reason for Codex.
        let resp = HookResponse::new(Decision::Ask, HookEventType::PreToolUse, AgentType::Codex);
        let (json, exit_code) = pre_tool_use_stdout(&resp);
        assert_eq!(exit_code, 0);
        let json = json.expect("Codex Ask must emit an explicit decision, not empty stdout");
        assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            json["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Codex")
        );

        // Claude Code still defers to its native prompt.
        let resp = HookResponse::new(
            Decision::Ask,
            HookEventType::PreToolUse,
            AgentType::ClaudeCode,
        );
        let (json, exit_code) = pre_tool_use_stdout(&resp);
        assert_eq!(exit_code, 0);
        assert_eq!(
            json.unwrap()["hookSpecificOutput"]["permissionDecision"],
            "ask"
        );
    }

    #[test]
    fn always_deferred_tool_emits_no_decision_on_permission_request() {
        // itr#388: an always-deferred tool reaching the PermissionRequest event
        // must defer to Claude's native dialog. Decision::Ask produces no decision
        // object (empty stdout), NOT a behavior:allow that silently resolves the
        // prompt with no selection ("Allowed by PermissionRequest hook").
        let response = HookResponse::new(
            Decision::Ask,
            HookEventType::PermissionRequest,
            AgentType::ClaudeCode,
        );
        assert!(permission_decision_object(&response).is_none());
        assert!(permission_response_value(&response).is_none());
    }

    #[test]
    fn dangerous_posture_never_swallows_interactive_prompts() {
        // The dangerous posture may auto-approve tool CALLS, but interactive
        // prompts (questions/plan/elicit) can't be "approved" — answering one
        // needs the human — so they must defer even here. Only operator
        // `always_ask` additions are released by the dangerous posture.
        let dir = dir_with_config(serde_json::json!({
            "auto_approve_level": "all",
            "auto_approve_dangerous": true,
            "always_ask": ["Bash"],
        }));
        assert!(is_always_deferred("AskUserQuestion", dir.path()));
        assert!(is_always_deferred("ExitPlanMode", dir.path()));
        assert!(is_always_deferred("Elicitation", dir.path()));
        // Operator-added harmful tool IS released by the dangerous posture.
        assert!(!is_always_deferred("Bash", dir.path()));
    }

    #[test]
    fn always_ask_remove_cannot_opt_out_interactive_prompts() {
        // `always_ask_remove` may drop operator `always_ask` additions, but never
        // an intrinsic interactive prompt — those defer unconditionally.
        let dir = dir_with_config(serde_json::json!({
            "always_ask": ["Bash"],
            "always_ask_remove": ["ExitPlanMode", "Bash"],
        }));
        // Intrinsic prompts ignore the removal.
        assert!(is_always_deferred("ExitPlanMode", dir.path()));
        assert!(is_always_deferred("AskUserQuestion", dir.path()));
        // Operator addition CAN be removed.
        assert!(!is_always_deferred("Bash", dir.path()));
    }

    #[test]
    fn always_ask_adds_a_custom_harmful_tool() {
        let dir = dir_with_config(serde_json::json!({
            "always_ask": ["Bash"],
        }));
        assert!(is_always_deferred("Bash", dir.path()));
    }

    #[test]
    fn default_auto_approve_no_longer_includes_questions() {
        // Removed from the read-tier defaults so the always-defer guard is the
        // single source of truth for these.
        let dir = tempfile::tempdir().unwrap();
        for tool in ["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"] {
            assert!(
                !is_auto_approved(tool, &serde_json::Value::Null, dir.path()),
                "{tool} must not be auto-approved by default"
            );
        }
    }

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

    // ---- itr#83: capped daemon-response line read ----

    #[test]
    fn capped_line_reads_normal_response() {
        // A normal newline-terminated daemon line passes and keeps its '\n',
        // matching std read_line semantics.
        let mut reader = std::io::BufReader::new(Cursor::new("{\"ok\":true}\nnext"));
        let mut line = String::new();
        let n = read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).unwrap();
        assert_eq!(line, "{\"ok\":true}\n");
        assert_eq!(n, "{\"ok\":true}\n".len());
    }

    #[test]
    fn capped_line_returns_zero_on_eof() {
        let mut reader = std::io::BufReader::new(Cursor::new(""));
        let mut line = String::new();
        let n = read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).unwrap();
        assert_eq!(n, 0);
        assert!(line.is_empty());
    }

    #[test]
    fn capped_line_rejects_over_limit_without_newline() {
        // A daemon that streams bytes with no newline past the cap is rejected
        // as an error (bounded memory), not buffered until OOM.
        let oversized = "z".repeat(64);
        let mut reader = std::io::BufReader::new(Cursor::new(oversized));
        let mut line = String::new();
        let err = read_line_capped(&mut reader, &mut line, 16).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        // The accumulated line never exceeds the cap.
        assert!(line.len() <= 16);
    }

    #[test]
    fn capped_line_accepts_line_at_exact_limit() {
        // 15 data bytes + '\n' == 16 == the cap: legitimate, must pass.
        let payload = format!("{}\n", "y".repeat(15));
        let mut reader = std::io::BufReader::new(Cursor::new(payload.clone()));
        let mut line = String::new();
        let n = read_line_capped(&mut reader, &mut line, 16).unwrap();
        assert_eq!(n, 16);
        assert_eq!(line, payload);
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

    // ── Control-plane self-protection (itr#425) ────────────────────────────

    /// A `<home>/.wisphive` layout so tilde forms expand the way they do in
    /// production (where `wisphive_dir.parent()` is the real home).
    fn home_and_state() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join(".wisphive");
        std::fs::create_dir_all(&state).unwrap();
        (home, state)
    }

    fn write_cmd(path: &str) -> serde_json::Value {
        json!({ "file_path": path })
    }

    #[test]
    fn self_protect_matches_absolute_state_path() {
        let (_home, state) = home_and_state();
        let target = state.join("config.json");
        assert!(targets_control_plane(
            "Write",
            &write_cmd(target.to_str().unwrap()),
            &state
        ));
    }

    #[test]
    fn self_protect_matches_tilde_and_home_forms() {
        let (_home, state) = home_and_state();
        for raw in ["~/.wisphive/mode", "$HOME/.wisphive/mode", "${HOME}/.wisphive/mode"] {
            assert!(
                targets_control_plane("Edit", &write_cmd(raw), &state),
                "expected {raw} to resolve into the state dir"
            );
        }
    }

    #[test]
    fn self_protect_covers_every_file_tool_and_notebook_path() {
        let (_home, state) = home_and_state();
        assert!(targets_control_plane("MultiEdit", &write_cmd("~/.wisphive/config.json"), &state));
        assert!(targets_control_plane(
            "NotebookEdit",
            &json!({ "notebook_path": "~/.wisphive/x.ipynb" }),
            &state
        ));
    }

    #[test]
    fn self_protect_collapses_parent_dir_escape() {
        let (_home, state) = home_and_state();
        // `..` back-and-forth must still resolve inside the state dir.
        assert!(targets_control_plane(
            "Write",
            &write_cmd("~/.wisphive/../.wisphive/config.json"),
            &state
        ));
    }

    #[test]
    fn self_protect_ignores_unrelated_and_sibling_paths() {
        let (home, state) = home_and_state();
        // A normal project edit.
        assert!(!targets_control_plane("Write", &write_cmd("/src/app.rs"), &state));
        // Component-wise containment: `.wisphive-evil` is NOT inside `.wisphive`.
        let sibling = home.path().join(".wisphive-evil").join("config.json");
        assert!(!targets_control_plane("Write", &write_cmd(sibling.to_str().unwrap()), &state));
    }

    #[test]
    fn self_protect_bash_substring_backstop() {
        let (_home, state) = home_and_state();
        assert!(targets_control_plane(
            "Bash",
            &json!({ "command": "echo off > ~/.wisphive/mode" }),
            &state
        ));
        let abs = state.join("config.json");
        assert!(targets_control_plane(
            "Bash",
            &json!({ "command": format!("cat {}", abs.display()) }),
            &state
        ));
        assert!(!targets_control_plane(
            "Bash",
            &json!({ "command": "cargo test --workspace" }),
            &state
        ));
    }

    #[test]
    fn self_protect_bypassed_by_opt_in_flag() {
        let (_home, state) = home_and_state();
        // Default: flag absent → guard on.
        assert!(!allow_self_modification(&state));
        // The composite gate the hook computes.
        let gated = |cfg: serde_json::Value| {
            std::fs::write(state.join("config.json"), cfg.to_string()).unwrap();
            !allow_self_modification(&state)
                && targets_control_plane("Write", &write_cmd("~/.wisphive/config.json"), &state)
        };
        assert!(gated(json!({})), "no flag → force human review");
        assert!(gated(json!({ "allow_self_modification": false })), "false → force review");
        assert!(!gated(json!({ "allow_self_modification": true })), "opt-in → allow");
    }
}
