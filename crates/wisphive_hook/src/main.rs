use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use wisphive_protocol::{
    ClientMessage, ClientType, Decision, DecisionRequest, PROTOCOL_VERSION, ServerMessage,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

fn main() {
    // Any failure = exit 0 (allow). Wisphive is fail-open.
    let code = match run() {
        Ok(Decision::Approve) => 0,
        Ok(Decision::Deny) => 1,
        Err(_) => 0,
    };
    process::exit(code);
}

fn run() -> Result<Decision, Box<dyn std::error::Error>> {
    let home = home_dir();

    // Layer 1: Mode file check
    let mode_path = home.join(".wisphive").join("mode");
    let mode = std::fs::read_to_string(&mode_path).unwrap_or_else(|_| "off".into());
    if mode.trim() != "active" {
        return Ok(Decision::Approve);
    }

    // Layer 2: Read Claude Code hook data from stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    // Parse the hook event — Claude Code sends JSON with tool info
    let hook_event: serde_json::Value = serde_json::from_str(&input)?;

    let tool_name = hook_event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let tool_input = hook_event
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // Derive agent_id from environment or generate one
    let agent_id =
        std::env::var("WISPHIVE_AGENT_ID").unwrap_or_else(|_| format!("cc-{}", process::id()));

    // Derive project from the current working directory
    let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let request = DecisionRequest {
        id: uuid::Uuid::new_v4(),
        agent_id,
        agent_type: wisphive_protocol::AgentType::ClaudeCode,
        project,
        tool_name,
        tool_input,
        timestamp: chrono::Utc::now(),
    };

    // Layer 3: Connect to daemon socket (fails instantly if daemon is dead)
    let socket_path = home.join(".wisphive").join("wisphive.sock");
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

    // Block for response (daemon controls timeout on its side;
    // we set a local read timeout as a safety net)
    // Remove the short timeout — we trust the daemon's timeout (up to 1 hour)
    writer.set_read_timeout(None)?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;

    let response: ServerMessage = wisphive_protocol::decode(&response_line)?;

    match response {
        ServerMessage::DecisionResponse { decision, .. } => Ok(decision),
        _ => Ok(Decision::Approve), // Unexpected response = allow
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
