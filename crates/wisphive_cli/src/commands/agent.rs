use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use wisphive_protocol::{
    ClientMessage, ClientType, ManagedAgent, ServerMessage, SpawnAgentRequest, PROTOCOL_VERSION,
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

/// Send a message and read one response.
fn send_and_recv(msg: &ClientMessage) -> Result<ServerMessage> {
    let (mut reader, mut writer) = connect_to_daemon()?;

    // Drain the initial QueueSnapshot that handle_tui sends
    let mut snapshot_line = String::new();
    reader.read_line(&mut snapshot_line)?;

    let encoded = wisphive_protocol::encode(msg)?;
    writer.write_all(encoded.as_bytes())?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    let response: ServerMessage = wisphive_protocol::decode(&response_line)?;
    Ok(response)
}

/// Start an agent process via the daemon.
pub async fn start(
    project: Option<PathBuf>,
    model: Option<String>,
    prompt: String,
    name: Option<String>,
) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    let project = std::fs::canonicalize(&project)
        .unwrap_or_else(|_| project.clone());

    // Pre-flight checks
    preflight_checks(&project)?;

    let request = SpawnAgentRequest {
        project: project.clone(),
        prompt,
        model: model.clone(),
        name: name.clone(),
    };

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
        ServerMessage::AgentExited { agent_id, exit_code } => {
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
fn preflight_checks(project: &PathBuf) -> Result<()> {
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
        anyhow::bail!(
            "Daemon is not running (no socket found).\n  fix: wisphive daemon start"
        );
    }
    let pid_path = wisphive_dir.join("wisphive.pid");
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                #[cfg(unix)]
                {
                    let alive = unsafe { libc::kill(pid, 0) } == 0;
                    if !alive {
                        anyhow::bail!(
                            "Daemon is not running (stale PID file).\n  fix: wisphive daemon start"
                        );
                    }
                }
            }
        }
    }

    // 3. Check hooks are installed in the project
    let settings_path = project.join(".claude").join("settings.json");
    if !settings_path.exists() {
        anyhow::bail!(
            "No .claude/settings.json in {}.\n  fix: wisphive hooks install --project {}",
            project.display(),
            project.display()
        );
    }
    // Verify wisphive hook is actually present
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if !content.contains("wisphive") {
            anyhow::bail!(
                "Wisphive hooks not installed in {}.\n  fix: wisphive hooks install --project {}",
                project.display(),
                project.display()
            );
        }
    }

    Ok(())
}

fn print_agent(agent: &ManagedAgent) {
    let project_name = agent
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| agent.project.display().to_string());

    eprintln!("  ID:      {}", agent.agent_id);
    eprintln!("  PID:     {}", agent.pid);
    eprintln!("  Project: {} ({})", project_name, agent.project.display());
    if let Some(ref model) = agent.model {
        eprintln!("  Model:   {}", model);
    }
    if let Some(ref name) = agent.name {
        eprintln!("  Name:    {}", name);
    }
    eprintln!("  Started: {}", agent.started_at.format("%Y-%m-%d %H:%M:%S UTC"));
}
