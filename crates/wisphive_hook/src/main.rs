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

/// Tools that are always safe to auto-approve (read-only, no side effects).
/// These never hit the daemon — the hook exits 0 immediately.
const DEFAULT_AUTO_APPROVE: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "WebSearch",
    "WebFetch",
    "NotebookRead",
    "Agent",
    "Skill",
    "TaskCreate",
    "TaskUpdate",
    "TaskGet",
    "TaskList",
    "TodoRead",
    "ToolSearch",
];

/// Hook response to format for Claude Code.
struct HookResponse {
    decision: Decision,
    message: Option<String>,
    updated_input: Option<serde_json::Value>,
    additional_context: Option<String>,
}

impl HookResponse {
    fn simple(decision: Decision) -> Self {
        Self { decision, message: None, updated_input: None, additional_context: None }
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

fn run() -> Result<HookResponse, Box<dyn std::error::Error>> {
    let home = home_dir();
    let wisphive_dir = home.join(".wisphive");

    // Layer 1: Mode file check
    let mode_path = wisphive_dir.join("mode");
    let mode = std::fs::read_to_string(&mode_path).unwrap_or_else(|_| "off".into());
    if mode.trim() != "active" {
        return Ok(HookResponse { decision: Decision::Approve, message: None, updated_input: None, additional_context: None });
    }

    // Layer 2: Read Claude Code hook data from stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let hook_event: serde_json::Value = serde_json::from_str(&input)?;

    // PostToolUse detection: Claude Code sends "tool_response" for post-execution results
    if hook_event.get("tool_response").is_some() {
        handle_post_tool_use(&hook_event, &wisphive_dir)?;
        return Ok(HookResponse::simple(Decision::Approve));
    }

    let tool_name = hook_event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Layer 3: Auto-approve safe tools before contacting daemon.
    // Check user overrides first (~/.wisphive/auto-approve.json), fall back to defaults.
    if is_auto_approved(&tool_name, &wisphive_dir) {
        return Ok(HookResponse::simple(Decision::Approve));
    }

    let tool_input = hook_event
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

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

    let request = DecisionRequest {
        id: uuid::Uuid::new_v4(),
        agent_id,
        agent_type: wisphive_protocol::AgentType::ClaudeCode,
        project,
        tool_name,
        tool_input,
        timestamp: chrono::Utc::now(),
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
            ..
        } => Ok(HookResponse {
            decision,
            message,
            updated_input,
            additional_context,
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
    }))?;
    writer.write_all(msg.as_bytes())?;

    Ok(())
}

/// Check if a tool is auto-approved.
///
/// Reads ~/.wisphive/auto-approve.json if it exists:
/// ```json
/// {
///   "auto_approve": ["Read", "Glob", "Grep", "Bash"],
///   "always_queue": ["Write", "Edit"]
/// }
/// ```
///
/// If the file doesn't exist, uses DEFAULT_AUTO_APPROVE.
fn is_auto_approved(tool_name: &str, wisphive_dir: &std::path::Path) -> bool {
    let config_path = wisphive_dir.join("auto-approve.json");

    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(arr) = config.get("auto_approve").and_then(|v| v.as_array()) {
                    return arr
                        .iter()
                        .any(|v| v.as_str().is_some_and(|s| s == tool_name));
                }
            }
        }
        // Config exists but is broken — fall through to defaults
    }

    DEFAULT_AUTO_APPROVE.iter().any(|&safe| safe == tool_name)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
