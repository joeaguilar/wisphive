use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use wisphive_daemon::project_audit::{AgentHookAudit, audit_project};
use wisphive_protocol::{
    AgentType, ClientCommand, ClientMessage, ClientType, ManagedAgent, PROTOCOL_VERSION,
    ServerMessage, SpawnAgentRequest,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect to the daemon socket and perform the Hello handshake.
fn connect_to_daemon() -> Result<(BufReader<UnixStream>, UnixStream)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let socket_path = PathBuf::from(home).join(".wisphive").join("wisphive.sock");

    connect_to_socket(&socket_path)
}

fn connect_to_socket(socket_path: &Path) -> Result<(BufReader<UnixStream>, UnixStream)> {
    let stream = UnixStream::connect(socket_path)
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
    send_and_recv_on(msg, uuid::Uuid::new_v4().to_string(), connect_to_daemon()?)
}

fn send_and_recv_on(
    msg: &ClientMessage,
    correlation_id: String,
    (mut reader, mut writer): (BufReader<UnixStream>, UnixStream),
) -> Result<ServerMessage> {
    let command = ClientCommand::from(msg.clone()).with_correlation_id(correlation_id.clone());
    let encoded = wisphive_protocol::encode(&command)?;
    writer.write_all(encoded.as_bytes())?;

    // handle_tui pushes a burst of snapshot messages on connect (AgentsSnapshot,
    // QueueSnapshot, AuditSnapshot — and possibly more later). Skip any of them
    // rather than draining a hard-coded count: that count silently grew from 2 to
    // 3 when AuditSnapshot was added and made every agent command misread the
    // AuditSnapshot as its response (itr#468).
    //
    // This connects as a Tui client, so the daemon may also interleave broadcast
    // events on this socket. Only accept a reply whose daemon-echoed correlation
    // ID matches this command; skip every other event, including a same-variant
    // broadcast from another client.
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
        if is_matching_agent_reply(&response, &correlation_id) {
            return Ok(response);
        }
        if matches!(response, ServerMessage::Error { .. }) {
            // Errors are written directly to this connection, never broadcast.
            return Ok(response);
        }
    }
}

fn is_matching_agent_reply(msg: &ServerMessage, correlation_id: &str) -> bool {
    match msg {
        ServerMessage::AgentSpawnQueued {
            correlation_id: Some(id),
            ..
        }
        | ServerMessage::AgentList {
            correlation_id: Some(id),
            ..
        }
        | ServerMessage::AgentStopResponse {
            correlation_id: Some(id),
            ..
        } => id == correlation_id,
        _ => false,
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
            anyhow::bail!("failed to start agent: {message}");
        }
        ServerMessage::AgentSpawnQueued { decision, .. } => {
            eprintln!(
                "Agent spawn queued for human approval (decision {}, project {}).",
                decision.id,
                decision.project.display()
            );
        }
        other => {
            anyhow::bail!("unexpected response while starting agent: {other:?}");
        }
    }

    Ok(())
}

/// List running agent processes.
pub async fn list() -> Result<()> {
    let response = send_and_recv(&ClientMessage::ListAgents)?;

    match response {
        ServerMessage::AgentList { agents, .. } => {
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
        ServerMessage::AgentStopResponse {
            agent_id,
            exit_code,
            ..
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

    // 3. Require the security-critical PreToolUse gate, but distinguish it
    // from a complete hook install. The shared audit layer uses the same exact
    // wisphive-hook matcher as install/status (itr#411), so partial telemetry
    // coverage can warn without weakening the existing fail-closed gate.
    if let Some(warning) = check_agent_hook_coverage(project, agent_type)? {
        eprintln!("{warning}");
    }

    Ok(())
}

fn check_agent_hook_coverage(project: &Path, agent_type: &AgentType) -> Result<Option<String>> {
    let project_audit = audit_project(project);
    let (agent_name, audit): (&str, &AgentHookAudit) = match agent_type {
        AgentType::ClaudeCode => ("Claude Code", &project_audit.hooks.claude),
        AgentType::Codex => ("Codex", &project_audit.hooks.codex),
        AgentType::Red | AgentType::LocalLlm => {
            anyhow::bail!("managed spawn currently supports Claude Code and Codex")
        }
    };

    if !audit.config_present {
        anyhow::bail!(
            "No {} in {}.\n  fix: wisphive hooks install --project {}",
            audit.config_path.display(),
            project.display(),
            project.display()
        );
    }

    if !audit.config_valid {
        if let Some(error) = audit.read_error.as_deref() {
            anyhow::bail!("could not read {}: {error}", audit.config_path.display());
        }
        anyhow::bail!(
            "{} is malformed JSON — fix its syntax before starting an agent",
            audit.config_path.display()
        );
    }

    let gated = audit
        .installed_events
        .iter()
        .any(|event| event == "PreToolUse");
    if !gated {
        anyhow::bail!(
            "Wisphive PreToolUse hook not installed in {}.\n  fix: wisphive hooks install --project {}",
            project.display(),
            project.display()
        );
    }

    if audit.installed {
        return Ok(None);
    }

    let installed = audit.installed_events.len();
    let total = installed + audit.missing_events.len();
    Ok(Some(format!(
        "WARN  {agent_name} is safely gated, but hooks are only partially installed ({installed}/{total} expected events).\n      missing expected events: {}\n      fix: wisphive hooks install --project {}",
        audit.missing_events.join(", "),
        project.display()
    )))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixListener;

    use super::*;
    use wisphive_daemon::hook_install::install_hooks;
    use wisphive_daemon::project_audit::{CLAUDE_HOOK_EVENTS, CODEX_HOOK_EVENTS};

    fn write_hook_settings(path: &Path, events: &[&str], command: &str) {
        let hooks = events
            .iter()
            .map(|event| {
                (
                    (*event).to_string(),
                    serde_json::json!([{
                        "matcher": "",
                        "hooks": [{ "type": "command", "command": command }]
                    }]),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({ "hooks": hooks })).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn pretool_only_configs_warn_with_every_missing_event_for_both_agents() {
        let project = tempfile::tempdir().unwrap();
        write_hook_settings(
            &project.path().join(".claude/settings.json"),
            &["PreToolUse"],
            "wisphive-hook",
        );
        write_hook_settings(
            &project.path().join(".codex/hooks.json"),
            &["PreToolUse"],
            "env WISPHIVE_AGENT_TYPE=codex wisphive-hook",
        );

        for (agent_type, expected) in [
            (AgentType::ClaudeCode, CLAUDE_HOOK_EVENTS),
            (AgentType::Codex, CODEX_HOOK_EVENTS),
        ] {
            let warning = check_agent_hook_coverage(project.path(), &agent_type)
                .unwrap()
                .expect("PreToolUse-only coverage must warn, not fail");
            assert!(warning.contains("safely gated"));
            assert!(warning.contains("wisphive hooks install --project"));
            for event in expected
                .iter()
                .copied()
                .filter(|event| *event != "PreToolUse")
            {
                assert!(warning.contains(event), "missing {event} in: {warning}");
            }
        }
    }

    #[test]
    fn full_hook_installs_pass_without_warnings_for_both_agents() {
        let project = tempfile::tempdir().unwrap();
        install_hooks(project.path()).unwrap();

        assert!(
            check_agent_hook_coverage(project.path(), &AgentType::ClaudeCode)
                .unwrap()
                .is_none()
        );
        assert!(
            check_agent_hook_coverage(project.path(), &AgentType::Codex)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn absent_and_malformed_hook_configs_fail_with_actionable_errors() {
        let project = tempfile::tempdir().unwrap();
        for agent_type in [AgentType::ClaudeCode, AgentType::Codex] {
            let error = check_agent_hook_coverage(project.path(), &agent_type).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("wisphive hooks install --project"));
            assert!(message.contains(project.path().to_string_lossy().as_ref()));
        }

        fs::create_dir_all(project.path().join(".claude")).unwrap();
        fs::create_dir_all(project.path().join(".codex")).unwrap();
        fs::write(project.path().join(".claude/settings.json"), "{").unwrap();
        fs::write(project.path().join(".codex/hooks.json"), "{").unwrap();
        for agent_type in [AgentType::ClaudeCode, AgentType::Codex] {
            let error = check_agent_hook_coverage(project.path(), &agent_type).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("malformed JSON"));
            assert!(message.contains("fix its syntax"));
        }
    }

    #[test]
    fn list_agents_skips_snapshots_and_interleaved_broadcast_before_own_reply() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("fake-daemon.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        let fake_daemon = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            let mut hello_line = String::new();
            reader.read_line(&mut hello_line).unwrap();
            let hello: ClientMessage = wisphive_protocol::decode(&hello_line).unwrap();
            assert!(matches!(
                hello,
                ClientMessage::Hello {
                    client: ClientType::Tui,
                    version: PROTOCOL_VERSION
                }
            ));

            for message in [
                ServerMessage::Welcome {
                    version: PROTOCOL_VERSION,
                },
                ServerMessage::AgentsSnapshot { agents: Vec::new() },
                ServerMessage::QueueSnapshot { items: Vec::new() },
            ] {
                stream
                    .write_all(wisphive_protocol::encode(&message).unwrap().as_bytes())
                    .unwrap();
            }

            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let request: ClientCommand = wisphive_protocol::decode(&request_line).unwrap();
            assert!(matches!(request.body, ClientMessage::ListAgents));
            let correlation_id = request
                .correlation_id
                .expect("CLI request must be correlated");

            // Simulate a same-variant AgentList fan-out from another client's
            // action arriving before our direct response.
            stream
                .write_all(
                    wisphive_protocol::encode(&ServerMessage::AgentList {
                        agents: vec![ManagedAgent {
                            agent_id: "broadcast-agent".into(),
                            agent_type: AgentType::Codex,
                            pid: 123,
                            project: PathBuf::from("/tmp/other-project"),
                            model: None,
                            name: None,
                            started_at: chrono::Utc::now(),
                            reasoning: None,
                            max_turns: None,
                            permission_mode: None,
                        }],
                        correlation_id: None,
                    })
                    .unwrap()
                    .as_bytes(),
                )
                .unwrap();

            stream
                .write_all(
                    wisphive_protocol::encode(&ServerMessage::AgentList {
                        agents: Vec::new(),
                        correlation_id: Some(correlation_id),
                    })
                    .unwrap()
                    .as_bytes(),
                )
                .unwrap();
        });

        let connection = connect_to_socket(&socket_path).unwrap();
        let response = send_and_recv_on(
            &ClientMessage::ListAgents,
            "our-list-request".into(),
            connection,
        )
        .unwrap();

        assert!(
            matches!(response, ServerMessage::AgentList { agents, correlation_id: Some(id) } if agents.is_empty() && id == "our-list-request")
        );
        fake_daemon.join().unwrap();
    }
}
