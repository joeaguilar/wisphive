use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use wisphive_protocol::{
    AgentType, ClientMessage, ClientType, ManagedAgent, PROTOCOL_VERSION, ServerMessage,
    SpawnAgentRequest,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect to the daemon socket and perform the Hello handshake.
fn connect_to_daemon() -> Result<(BufReader<UnixStream>, UnixStream)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let socket_path = PathBuf::from(home).join(".wisphive").join("wisphive.sock");

    let stream = UnixStream::connect(&socket_path)
        .context("could not connect to daemon — is it running? (wisphive daemon start)")?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    // Handshake
    let hello = wisphive_protocol::encode(&ClientMessage::Hello {
        client: ClientType::Tui, // reuse TUI client type for CLI agent commands
        version: PROTOCOL_VERSION,
    })?;
    writer.write_all(hello.as_bytes())?;

    let mut welcome_line = String::new();
    reader.read_line(&mut welcome_line)?;
    let msg: ServerMessage = wisphive_protocol::decode(&welcome_line)?;
    match msg {
        ServerMessage::Welcome { .. } => {}
        ServerMessage::Error { message } => anyhow::bail!("daemon error: {message}"),
        _ => anyhow::bail!("unexpected response from daemon"),
    }

    Ok((reader, writer))
}

/// True for the snapshot messages the daemon proactively pushes to a TUI-type
/// client on connect. These arrive before the reply to our request and must be
/// skipped over.
fn is_connect_snapshot(msg: &ServerMessage) -> bool {
    matches!(
        msg,
        ServerMessage::AgentsSnapshot { .. }
            | ServerMessage::QueueSnapshot { .. }
            | ServerMessage::AuditSnapshot { .. }
    )
}

/// Send a message and read one response.
fn send_and_recv(msg: &ClientMessage) -> Result<ServerMessage> {
    let (mut reader, mut writer) = connect_to_daemon()?;

    let encoded = wisphive_protocol::encode(msg)?;
    writer.write_all(encoded.as_bytes())?;

    // handle_tui pushes a burst of snapshot messages on connect (AgentsSnapshot,
    // QueueSnapshot, AuditSnapshot — and possibly more later). Skip any of them
    // rather than draining a hard-coded count: that count silently grew from 2 to
    // 3 when AuditSnapshot was added and made every agent command misread the
    // AuditSnapshot as its response (itr#468).
    //
    // Caveat (itr#470): this connects as a Tui client, so the daemon may also
    // interleave *broadcast* events on this socket. Those are not skipped here, so
    // a concurrent event of the same variant as the reply can still be misread.
    // The real fix is request/response correlation or a non-subscribed CLI client
    // type; tracked separately.
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            anyhow::bail!("daemon closed the connection before responding");
        }
        let response: ServerMessage = wisphive_protocol::decode(&line)?;
        if is_connect_snapshot(&response) {
            continue;
        }
        return Ok(response);
    }
}

/// Start an agent process via the daemon.
pub async fn start(req: SpawnAgentRequest) -> Result<()> {
    let project = std::fs::canonicalize(&req.project).unwrap_or_else(|_| req.project.clone());

    // Pre-flight checks
    preflight_checks(&project, &req.agent_type)?;

    let request = SpawnAgentRequest { project, ..req };
    let response = send_and_recv(&ClientMessage::SpawnAgent(request))?;

    match response {
        ServerMessage::AgentSpawned(agent) => {
            eprintln!("Agent started:");
            print_agent(&agent);
        }
        ServerMessage::Error { message } => {
            eprintln!("Failed to start agent: {message}");
        }
        other => {
            eprintln!("Unexpected response: {:?}", other);
        }
    }

    Ok(())
}

/// List running agent processes.
pub async fn list() -> Result<()> {
    let response = send_and_recv(&ClientMessage::ListAgents)?;

    match response {
        ServerMessage::AgentList { agents } => {
            if agents.is_empty() {
                eprintln!("No managed agents running.");
            } else {
                for agent in &agents {
                    print_agent(agent);
                    eprintln!();
                }
            }
        }
        ServerMessage::Error { message } => {
            eprintln!("Error: {message}");
        }
        other => {
            eprintln!("Unexpected response: {:?}", other);
        }
    }

    Ok(())
}

/// Stop an agent process.
pub async fn stop(agent_id: String) -> Result<()> {
    let response = send_and_recv(&ClientMessage::StopAgent {
        agent_id: agent_id.clone(),
    })?;

    match response {
        ServerMessage::AgentExited {
            agent_id,
            exit_code,
        } => {
            eprintln!(
                "Agent {} stopped (exit code: {})",
                agent_id,
                exit_code.map_or("unknown".into(), |c| c.to_string())
            );
        }
        ServerMessage::Error { message } => {
            eprintln!("Error: {message}");
        }
        other => {
            eprintln!("Unexpected response: {:?}", other);
        }
    }

    Ok(())
}

/// Verify the system is ready to spawn an agent.
fn preflight_checks(project: &Path, agent_type: &AgentType) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let wisphive_dir = PathBuf::from(&home).join(".wisphive");

    // 1. Check mode is active
    let mode_path = wisphive_dir.join("mode");
    let mode = std::fs::read_to_string(&mode_path).unwrap_or_else(|_| "off".into());
    if mode.trim() != "active" {
        anyhow::bail!(
            "Wisphive hooks are not active (mode: {}).\n  fix: wisphive hooks enable",
            mode.trim()
        );
    }

    // 2. Check daemon is running (socket exists and PID is alive)
    let socket_path = wisphive_dir.join("wisphive.sock");
    if !socket_path.exists() {
        anyhow::bail!("Daemon is not running (no socket found).\n  fix: wisphive daemon start");
    }
    let pid_path = wisphive_dir.join("wisphive.pid");
    if pid_path.exists()
        && let Ok(pid_str) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = pid_str.trim().parse::<i32>()
    {
        #[cfg(unix)]
        {
            if !process_exists(pid) {
                anyhow::bail!(
                    "Daemon is not running (stale PID file).\n  fix: wisphive daemon start"
                );
            }
        }
    }

    // 3. Check hooks are installed in the project
    let hooks_path = match agent_type {
        AgentType::Codex => project.join(".codex").join("hooks.json"),
        AgentType::ClaudeCode => project.join(".claude").join("settings.json"),
        AgentType::Red | AgentType::LocalLlm => {
            anyhow::bail!("managed spawn currently supports Claude Code and Codex")
        }
    };
    if !hooks_path.exists() {
        anyhow::bail!(
            "No {} in {}.\n  fix: wisphive hooks install --project {}",
            hooks_path.display(),
            project.display(),
            project.display()
        );
    }
    // Verify a wisphive hook is actually installed — matched precisely on the
    // wisphive-hook binary (itr#359), not a substring that a user hook under a
    // "wisphive" directory would satisfy. Fail closed: an unreadable or
    // malformed hook file blocks the spawn rather than skipping the check,
    // and the hook must be on PreToolUse — the event that actually gates
    // tool calls — not merely on some telemetry event.
    let content = std::fs::read_to_string(&hooks_path)
        .with_context(|| format!("could not read {}", hooks_path.display()))?;
    let settings: serde_json::Value = serde_json::from_str(&content).with_context(|| {
        format!(
            "{} is malformed JSON — fix its syntax before starting an agent",
            hooks_path.display()
        )
    })?;
    let installed = settings
        .get("hooks")
        .and_then(|hooks| hooks.get("PreToolUse"))
        .and_then(|rules| rules.as_array())
        .is_some_and(|arr| {
            arr.iter()
                .any(wisphive_daemon::hook_install::has_wisphive_hook)
        });
    if !installed {
        anyhow::bail!(
            "Wisphive PreToolUse hook not installed in {}.\n  fix: wisphive hooks install --project {}",
            project.display(),
            project.display()
        );
    }

    Ok(())
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn print_agent(agent: &ManagedAgent) {
    let project_name = agent
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| agent.project.display().to_string());

    eprintln!("  ID:      {}", agent.agent_id);
    eprintln!("  Type:    {}", agent.agent_type);
    eprintln!("  PID:     {}", agent.pid);
    eprintln!("  Project: {} ({})", project_name, agent.project.display());
    if let Some(ref model) = agent.model {
        eprintln!("  Model:   {}", model);
    }
    if let Some(ref name) = agent.name {
        eprintln!("  Name:    {}", name);
    }
    if let Some(ref reasoning) = agent.reasoning {
        eprintln!("  Reason:  {}", reasoning);
    }
    if let Some(max_turns) = agent.max_turns {
        eprintln!("  Turns:   {}", max_turns);
    }
    if let Some(ref perm) = agent.permission_mode {
        eprintln!("  PermMod: {}", perm);
    }
    eprintln!(
        "  Started: {}",
        agent.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
}
