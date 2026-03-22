use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use wisphive_protocol::{
    ClientMessage, ClientType, Decision, DecisionRequest, PROTOCOL_VERSION, ServerMessage,
    ToolResult,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

/// Tools that are always safe to auto-approve (read-only + orchestration).
/// Fallback when no config.json exists. Matches the Read tier.
const DEFAULT_AUTO_APPROVE: &[&str] = &[
    "Read", "Glob", "Grep", "LS", "LSP", "NotebookRead",
    "WebSearch", "WebFetch",
    "Agent", "Skill", "ToolSearch", "AskUserQuestion",
    "EnterPlanMode", "ExitPlanMode", "EnterWorktree", "ExitWorktree",
    "TaskCreate", "TaskUpdate", "TaskGet", "TaskList", "TaskOutput", "TaskStop", "TodoRead",
    "CronList",
];

/// Hook response to format for Claude Code.
struct HookResponse {
    decision: Decision,
    message: Option<String>,
    updated_input: Option<serde_json::Value>,
    additional_context: Option<String>,
    /// For PermissionRequest: the selected suggestion to echo back.
    selected_permission: Option<wisphive_protocol::PermissionSuggestion>,
    /// The hook event type — determines the response JSON format.
    event_type: wisphive_protocol::HookEventType,
}

impl HookResponse {
    fn simple(decision: Decision) -> Self {
        Self {
            decision,
            message: None,
            updated_input: None,
            additional_context: None,
            selected_permission: None,
            event_type: wisphive_protocol::HookEventType::PreToolUse,
        }
    }
}

fn main() {
    // Any failure = exit 0 (allow). Wisphive is fail-open.
    let code = match run() {
        Ok(resp) => format_and_exit(&resp),
        Err(_) => 0,
    };
    process::exit(code);
}

/// Format the hook response as Claude Code JSON stdout and return exit code.
fn format_and_exit(resp: &HookResponse) -> i32 {
    use wisphive_protocol::HookEventType::*;
    match resp.event_type {
        PermissionRequest => return format_permission_response(resp),
        Stop | SubagentStop => return format_stop_response(resp),
        UserPromptSubmit | ConfigChange => return format_block_response(resp),
        Elicitation => return format_elicitation_response(resp),
        TeammateIdle => return format_teammate_idle_response(resp),
        TaskCompleted => return format_task_completed_response(resp),
        _ => {} // PreToolUse and unknown fall through to existing logic
    }
    match resp.decision {
        Decision::Ask => {
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
                // Deny with feedback via JSON (Claude sees the reason)
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
            let has_extras = resp.updated_input.is_some() || resp.additional_context.is_some();
            if has_extras {
                let mut output = serde_json::Map::new();
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
                output.insert(
                    "hookSpecificOutput".into(),
                    serde_json::Value::Object(hook_output),
                );
                if let Some(ref ctx) = resp.additional_context {
                    output.insert(
                        "additionalContext".into(),
                        serde_json::Value::String(ctx.clone()),
                    );
                }
                print!("{}", serde_json::Value::Object(output));
            }
            0
        }
    }
}

/// Format a PermissionRequest response for Claude Code.
fn format_permission_response(resp: &HookResponse) -> i32 {
    let mut decision_obj = serde_json::Map::new();

    match resp.decision {
        Decision::Approve => {
            decision_obj.insert("behavior".into(), serde_json::Value::String("allow".into()));
            if let Some(ref perm) = resp.selected_permission {
                decision_obj.insert(
                    "updatedPermissions".into(),
                    serde_json::to_value(vec![perm]).unwrap_or(serde_json::json!([])),
                );
            }
            if let Some(ref input) = resp.updated_input {
                decision_obj.insert("updatedInput".into(), input.clone());
            }
        }
        Decision::Deny => {
            decision_obj.insert("behavior".into(), serde_json::Value::String("deny".into()));
            if let Some(ref msg) = resp.message {
                decision_obj.insert("message".into(), serde_json::Value::String(msg.clone()));
            }
        }
        Decision::Ask => {
            // Defer — don't output anything, let native prompt handle it
            return 0;
        }
    }

    let json = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": serde_json::Value::Object(decision_obj)
        }
    });
    print!("{}", json);
    0
}

/// Format Stop/SubagentStop: approve = let stop (exit 0), deny = continue working.
fn format_stop_response(resp: &HookResponse) -> i32 {
    match resp.decision {
        Decision::Approve => {
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
        Decision::Approve => 0,
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
    hook_output.insert("hookEventName".into(), serde_json::json!("Elicitation"));
    hook_output.insert("action".into(), serde_json::json!(action));
    if action == "accept" {
        if let Some(ref input) = resp.updated_input {
            hook_output.insert("content".into(), input.clone());
        }
    }
    output.insert("hookSpecificOutput".into(), serde_json::Value::Object(hook_output));
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

fn run() -> Result<HookResponse, Box<dyn std::error::Error>> {
    let home = home_dir();
    let wisphive_dir = home.join(".wisphive");

    // Layer 1: Mode file check
    let mode_path = wisphive_dir.join("mode");
    let mode = std::fs::read_to_string(&mode_path).unwrap_or_else(|_| "off".into());
    if mode.trim() != "active" {
        return Ok(HookResponse::simple(Decision::Approve));
    }

    // Layer 2: Read Claude Code hook data from stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let hook_event: serde_json::Value = serde_json::from_str(&input)?;

    // Determine event type from hook_event_name (early — needed for dispatch)
    let event_type: wisphive_protocol::HookEventType = hook_event
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("PreToolUse")
        .parse()
        .unwrap_or_default();

    // PostToolUse detection: fire-and-forget result to daemon
    if event_type == wisphive_protocol::HookEventType::PostToolUse
        || hook_event.get("tool_response").is_some()
    {
        handle_post_tool_use(&hook_event, &wisphive_dir)?;
        return Ok(HookResponse::simple(Decision::Approve));
    }

    let is_permission_request = event_type == wisphive_protocol::HookEventType::PermissionRequest;

    let tool_name = hook_event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Extract agent identity early (needed for registration before auto-approve check)
    let agent_id = hook_event
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| format!("cc-{}", s))
        .or_else(|| std::env::var("WISPHIVE_AGENT_ID").ok())
        .unwrap_or_else(|| format!("cc-{}", process::id()));

    let project = std::env::var("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            hook_event
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .ok_or(())
        })
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let tool_input = hook_event
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tool_use_id = hook_event
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Layer 3: Register agent with daemon (once per session, fire-and-forget)
    register_agent_once(&agent_id, &project, &wisphive_dir);

    // Auto-approve Stop/SubagentStop events if configured
    if matches!(event_type, wisphive_protocol::HookEventType::Stop | wisphive_protocol::HookEventType::SubagentStop) {
        if is_stop_auto_approved(&wisphive_dir) {
            log_auto_approved(
                &wisphive_dir, &tool_use_id, &agent_id, &project, &tool_name, &tool_input, event_type,
            );
            return Ok(HookResponse::simple(Decision::Approve));
        }
    }

    // Auto-approve UserPromptSubmit and ConfigChange — routine events that don't need review
    if matches!(event_type, wisphive_protocol::HookEventType::UserPromptSubmit | wisphive_protocol::HookEventType::ConfigChange) {
        log_auto_approved(
            &wisphive_dir, &tool_use_id, &agent_id, &project, &tool_name, &tool_input, event_type,
        );
        return Ok(HookResponse::simple(Decision::Approve));
    }

    // Layer 4: Auto-approve check — PermissionRequests always go to daemon
    if !is_permission_request && is_auto_approved(&tool_name, &tool_input, &wisphive_dir) {
        log_auto_approved(
            &wisphive_dir, &tool_use_id, &agent_id, &project, &tool_name, &tool_input, event_type,
        );
        return Ok(HookResponse::simple(Decision::Approve));
    }

    // Parse permission suggestions for PermissionRequest events
    let permission_suggestions = if is_permission_request {
        hook_event
            .get("permission_suggestions")
            .and_then(|v| serde_json::from_value::<Vec<wisphive_protocol::PermissionSuggestion>>(v.clone()).ok())
    } else {
        None
    };

    // Extract event-specific data for non-PreToolUse events
    let mut event_data = extract_event_data(event_type, &hook_event);

    // For ExitPlanMode, extract plan content from transcript
    if tool_name == "ExitPlanMode" {
        if let Some(plan) = hook_event
            .get("transcript_path")
            .and_then(|v| v.as_str())
            .and_then(extract_plan_from_transcript)
        {
            let data = event_data.get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Some(obj) = data.as_object_mut() {
                obj.insert("plan_content".into(), serde_json::Value::String(plan));
            }
        }
    }

    let request = DecisionRequest {
        id: uuid::Uuid::new_v4(),
        agent_id,
        agent_type: wisphive_protocol::AgentType::ClaudeCode,
        project,
        tool_name,
        tool_input,
        timestamp: chrono::Utc::now(),
        hook_event_name: event_type,
        tool_use_id: tool_use_id.clone(),
        permission_suggestions,
        event_data,
    };

    // Layer 4: Connect to daemon socket (fails instantly if daemon is dead)
    let socket_path = wisphive_dir.join("wisphive.sock");
    let stream = UnixStream::connect(&socket_path)?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

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
    let _welcome: ServerMessage = wisphive_protocol::decode(&welcome_line)?;

    // Send decision request
    let req_msg = wisphive_protocol::encode(&ClientMessage::DecisionRequest(request))?;
    writer.write_all(req_msg.as_bytes())?;

    // Block for response — daemon controls timeout (up to 1 hour).
    writer.set_read_timeout(None)?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;

    let response: ServerMessage = wisphive_protocol::decode(&response_line)?;

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
        }),
        _ => Ok(HookResponse::simple(Decision::Approve)),
    }
}

/// Handle a PostToolUse event: fire-and-forget the result to the daemon.
fn handle_post_tool_use(
    hook_event: &serde_json::Value,
    wisphive_dir: &std::path::Path,
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
        .map(|s| format!("cc-{}", s))
        .or_else(|| std::env::var("WISPHIVE_AGENT_ID").ok())
        .unwrap_or_else(|| format!("cc-{}", std::process::id()));

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
fn register_agent_once(agent_id: &str, project: &std::path::Path, wisphive_dir: &std::path::Path) {
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
            agent_type: wisphive_protocol::AgentType::ClaudeCode,
            project: project.to_path_buf(),
        })?;
        writer.write_all(msg.as_bytes())?;

        // Create marker file
        let _ = std::fs::create_dir_all(&sessions_dir);
        let _ = std::fs::write(&marker, "");

        Ok(())
    })();
}

/// Check if Stop events should be auto-approved (config: `auto_approve_stop: true`).
fn is_stop_auto_approved(wisphive_dir: &std::path::Path) -> bool {
    let config_path = wisphive_dir.join("config.json");
    std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|config| config.get("auto_approve_stop")?.as_bool())
        .unwrap_or(false)
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
    if let Some(ref config) = config {
        if let Some(rules) = config.get("tool_rules").and_then(|v| v.as_object()) {
            if let Some(rule) = rules.get(tool_name) {
                let input_text = tool_input_text(tool_name, tool_input);
                let input_lower = input_text.to_lowercase();

                if base_approved {
                    // Check deny_patterns — any match blocks auto-approve
                    if let Some(patterns) = rule.get("deny_patterns").and_then(|v| v.as_array()) {
                        for p in patterns {
                            if let Some(pat) = p.as_str() {
                                if input_lower.contains(&pat.to_lowercase()) {
                                    return false;
                                }
                            }
                        }
                    }
                } else {
                    // Check allow_patterns — any match auto-approves
                    if let Some(patterns) = rule.get("allow_patterns").and_then(|v| v.as_array()) {
                        for p in patterns {
                            if let Some(pat) = p.as_str() {
                                if input_lower.contains(&pat.to_lowercase()) {
                                    return true;
                                }
                            }
                        }
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
    if let Some(arr) = config.get("auto_approve_add").and_then(|v| v.as_array()) {
        if arr.iter().any(|v| v.as_str() == Some(tool_name)) {
            return true;
        }
    }

    // Check tiered level
    if let Some(level_str) = config.get("auto_approve_level").and_then(|v| v.as_str()) {
        if let Ok(level) = level_str.parse::<wisphive_protocol::AutoApproveLevel>() {
            return level.includes(tool_name);
        }
    }

    // Fallback to legacy
    legacy_auto_approved(tool_name, wisphive_dir)
}

/// Check legacy auto-approve.json and built-in defaults.
fn legacy_auto_approved(tool_name: &str, wisphive_dir: &std::path::Path) -> bool {
    let legacy_path = wisphive_dir.join("auto-approve.json");
    if legacy_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&legacy_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = config.get("auto_approve").and_then(|v| v.as_array()) {
                    return arr.iter().any(|v| v.as_str() == Some(tool_name));
                }
            }
        }
    }
    DEFAULT_AUTO_APPROVE.iter().any(|&safe| safe == tool_name)
}

/// Log an auto-approved tool call to events.jsonl for daemon ingestion.
/// Uses O_APPEND for atomic writes (~0.1-1μs). All errors are swallowed (fail-open).
fn log_auto_approved(
    wisphive_dir: &std::path::Path,
    tool_use_id: &Option<String>,
    agent_id: &str,
    project: &std::path::Path,
    tool_name: &str,
    tool_input: &serde_json::Value,
    event_type: wisphive_protocol::HookEventType,
) {
    let path = wisphive_dir.join("events.jsonl");
    let entry = serde_json::json!({
        "event": "auto_approved",
        "hook_event_name": event_type.to_string(),
        "tool_use_id": tool_use_id,
        "agent_id": agent_id,
        "agent_type": "claude_code",
        "project": project,
        "tool_name": tool_name,
        "tool_input": tool_input,
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
    if tool_name == "Bash" {
        if let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) {
            return cmd.to_string();
        }
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
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

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
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(text.to_string());
                }
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
