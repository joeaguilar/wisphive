use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::process::{Child, Command};
use tracing::{error, info, warn};
use wisphive_protocol::{ManagedAgent, SpawnAgentRequest};

/// Tracks agent processes spawned by the daemon.
pub struct ProcessRegistry {
    processes: HashMap<String, ManagedProcess>,
}

struct ManagedProcess {
    child: Child,
    info: ManagedAgent,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
        }
    }

    /// Spawn a new Claude Code agent process.
    ///
    /// Returns the `ManagedAgent` metadata on success.
    pub async fn spawn_agent(&mut self, req: SpawnAgentRequest) -> Result<ManagedAgent> {
        let agent_id = format!("agent-{}", uuid::Uuid::new_v4().as_simple());
        let session_id = uuid::Uuid::new_v4();

        let mut cmd = Command::new("claude");

        // Non-interactive print mode
        cmd.arg("-p");

        // Wisphive is the gatekeeper — skip Claude's own permission prompts
        cmd.arg("--dangerously-skip-permissions");

        // Session tracking
        cmd.args(["--session-id", &session_id.to_string()]);

        if let Some(ref model) = req.model {
            cmd.args(["--model", model]);
        }

        if let Some(ref name) = req.name {
            cmd.args(["--name", name]);
        }

        // The prompt is the positional argument
        cmd.arg(&req.prompt);

        // Run in the project directory
        cmd.current_dir(&req.project);

        // Set env var for hook correlation
        cmd.env("WISPHIVE_AGENT_ID", &agent_id);

        // Capture stdout, pipe stderr through
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .context("failed to spawn claude — is it installed and on PATH?")?;

        let pid = child
            .id()
            .context("could not get PID of spawned process")?;

        let managed = ManagedAgent {
            agent_id: agent_id.clone(),
            pid,
            project: req.project,
            model: req.model,
            name: req.name,
            started_at: Utc::now(),
        };

        info!(
            agent_id = %agent_id,
            pid = pid,
            project = %managed.project.display(),
            "spawned agent process"
        );

        self.processes.insert(
            agent_id,
            ManagedProcess {
                child,
                info: managed.clone(),
            },
        );

        Ok(managed)
    }

    /// Stop an agent process by sending SIGTERM.
    pub async fn stop_agent(&mut self, agent_id: &str) -> Result<Option<i32>> {
        let Some(mut proc) = self.processes.remove(agent_id) else {
            anyhow::bail!("no managed agent with id: {agent_id}");
        };

        info!(agent_id = %agent_id, "stopping agent process");

        // Try graceful kill first
        if let Err(e) = proc.child.kill().await {
            warn!(agent_id = %agent_id, "kill failed: {e}");
        }

        let status = proc.child.wait().await?;
        let code = status.code();

        info!(agent_id = %agent_id, exit_code = ?code, "agent process stopped");
        Ok(code)
    }

    /// List all managed agent processes.
    pub fn list(&self) -> Vec<ManagedAgent> {
        self.processes
            .values()
            .map(|p| p.info.clone())
            .collect()
    }

    /// Reap any processes that have exited. Returns (agent_id, exit_code) pairs.
    pub async fn reap_exited(&mut self) -> Vec<(String, Option<i32>)> {
        let mut exited = Vec::new();

        for (id, proc) in &mut self.processes {
            match proc.child.try_wait() {
                Ok(Some(status)) => {
                    info!(agent_id = %id, exit_code = ?status.code(), "agent process exited");
                    exited.push((id.clone(), status.code()));
                }
                Ok(None) => {} // still running
                Err(e) => {
                    error!(agent_id = %id, "error checking process status: {e}");
                }
            }
        }

        for (id, _) in &exited {
            self.processes.remove(id);
        }

        exited
    }

    /// Kill all managed processes. Called during daemon shutdown.
    pub async fn shutdown_all(&mut self) {
        let ids: Vec<String> = self.processes.keys().cloned().collect();
        for id in ids {
            if let Err(e) = self.stop_agent(&id).await {
                warn!(agent_id = %id, "error stopping agent during shutdown: {e}");
            }
        }
    }

    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        Self::new()
    }
}
