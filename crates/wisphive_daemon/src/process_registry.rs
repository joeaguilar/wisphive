use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::process::{Child, Command};
use tracing::{error, info, warn};
use wisphive_protocol::{AgentType, ManagedAgent, SpawnAgentRequest};

/// Tracks agent processes spawned by the daemon.
pub struct ProcessRegistry {
    processes: HashMap<String, ManagedProcess>,
    /// itr#471: operator opt-in to allow managed Codex spawns into projects
    /// whose `.codex/hooks.json` carries non-Wisphive hooks (which the trust
    /// bypass would run headlessly). Fail-safe default is `false` (refuse).
    /// Captured from config at daemon start.
    codex_allow_foreign_hooks: bool,
}

struct ManagedProcess {
    child: Child,
    info: ManagedAgent,
}

impl ProcessRegistry {
    pub fn new(codex_allow_foreign_hooks: bool) -> Self {
        Self {
            processes: HashMap::new(),
            codex_allow_foreign_hooks,
        }
    }

    /// Spawn a new managed agent process.
    ///
    /// Returns the `ManagedAgent` metadata on success.
    pub async fn spawn_agent(&mut self, req: SpawnAgentRequest) -> Result<ManagedAgent> {
        let agent_id = format!("agent-{}", uuid::Uuid::new_v4().as_simple());
        let session_id = uuid::Uuid::new_v4();
        let agent_type = req.agent_type.clone();

        // itr#467: Codex silently SKIPS hooks it has not been granted persisted
        // trust for. We pass `--dangerously-bypass-hook-trust` below so the
        // daemon-installed (and therefore vetted) Wisphive hook actually runs —
        // but that bypass only gates anything if the hook is present. If the
        // project has no Wisphive Codex hook, a spawned agent would run
        // completely UNGATED while appearing "managed". Fail closed rather than
        // present an ungated agent as controlled.
        if matches!(agent_type, AgentType::Codex) {
            // Strict, fail-closed check (itr#467 review): matches the
            // wisphive-hook binary precisely, not a "wisphive" substring — the
            // web/loop/TUI spawn paths don't run the CLI preflight, so this
            // guard is their only gate and must be at least as strict.
            if !crate::hook_install::codex_pretooluse_hook_installed(&req.project) {
                anyhow::bail!(
                    "refusing to spawn Codex into {}: no Wisphive PreToolUse hook is \
                     installed there, so the agent's tool calls would bypass the control \
                     plane. Run `wisphive hooks install --project {}` first.",
                    req.project.display(),
                    req.project.display()
                );
            }

            // itr#471: `--dangerously-bypass-hook-trust` (passed below) suppresses
            // Codex's trust prompt for EVERY hook in the project's
            // `.codex/hooks.json`, not just Wisphive's. Refuse to run un-vetted
            // third-party hooks headlessly unless the operator opts in.
            let foreign = crate::hook_install::codex_foreign_hook_commands(&req.project);
            if !foreign.is_empty() {
                warn!(
                    project = %req.project.display(),
                    foreign_hooks = ?foreign,
                    "Codex managed spawn: non-Wisphive hook(s) present; \
                     --dangerously-bypass-hook-trust would run them headlessly"
                );
                if !self.codex_allow_foreign_hooks {
                    anyhow::bail!(
                        "refusing to spawn Codex into {}: its .codex/hooks.json carries \
                         non-Wisphive hook(s) [{}] that --dangerously-bypass-hook-trust \
                         would run headlessly without Codex's trust prompt. Review them, \
                         then set \"codex_allow_foreign_hooks\": true in \
                         ~/.wisphive/config.json to allow.",
                        req.project.display(),
                        foreign.join(", ")
                    );
                }
            }
        }

        let mut cmd = match &agent_type {
            AgentType::ClaudeCode => {
                let mut cmd = Command::new("claude");

                // Non-interactive print mode
                cmd.arg("-p");

                // Wisphive is the gatekeeper — skip Claude's own permission prompts
                cmd.arg("--dangerously-skip-permissions");

                // Session tracking — skip if resuming an existing session
                if !req.continue_session && req.resume.is_none() {
                    cmd.args(["--session-id", &session_id.to_string()]);
                }

                if let Some(ref model) = req.model {
                    cmd.args(["--model", model]);
                }
                if let Some(ref name) = req.name {
                    cmd.args(["--name", name]);
                }
                if let Some(ref reasoning) = req.reasoning {
                    cmd.args(["--reasoning", reasoning]);
                }
                if let Some(max_turns) = req.max_turns {
                    cmd.args(["--max-turns", &max_turns.to_string()]);
                }
                if let Some(ref perm_mode) = req.permission_mode {
                    cmd.args(["--permission-mode", perm_mode]);
                }
                if let Some(ref sys_prompt) = req.system_prompt {
                    cmd.args(["--system-prompt", sys_prompt]);
                }
                if let Some(ref append_prompt) = req.append_system_prompt {
                    cmd.args(["--append-system-prompt", append_prompt]);
                }
                if let Some(ref tools) = req.allowed_tools {
                    for tool in tools {
                        cmd.args(["--allowedTools", tool]);
                    }
                }
                if let Some(ref tools) = req.disallowed_tools {
                    for tool in tools {
                        cmd.args(["--disallowedTools", tool]);
                    }
                }
                if req.continue_session {
                    cmd.arg("--continue");
                }
                if let Some(ref session) = req.resume {
                    cmd.args(["--resume", session]);
                }
                if let Some(ref fmt) = req.output_format {
                    cmd.args(["--output-format", fmt]);
                }
                if req.verbose {
                    cmd.arg("--verbose");
                }

                cmd.arg(&req.prompt);
                cmd
            }
            AgentType::Codex => {
                let mut cmd = Command::new("codex");
                let project = req.project.display().to_string();

                cmd.arg("exec");
                // `codex exec` is already non-interactive and never prompts for
                // approval; the previous `--ask-for-approval never` is NOT a valid
                // `codex exec` flag and aborted the spawn at arg-parse (itr#467).
                cmd.args(["--sandbox", "workspace-write"]);
                // A daemon-controlled spawn targets an arbitrary project dir that
                // may not be a git repo; without this, codex refuses with "Not
                // inside a trusted directory" (itr#467).
                cmd.arg("--skip-git-repo-check");
                // Force the Wisphive hook to run without an interactive
                // `/hooks`-trust step. Codex skips untrusted hooks silently, which
                // would leave the agent UNGATED. This is the documented path for
                // "automation that already vets its hook sources" — the daemon
                // installs the hook itself, and the fail-closed check in
                // spawn_agent guarantees it is present before we get here (itr#467).
                cmd.arg("--dangerously-bypass-hook-trust");
                cmd.arg("-C");
                cmd.arg(&project);

                if let Some(ref model) = req.model {
                    cmd.args(["--model", model]);
                }
                if let Some(ref reasoning) = req.reasoning {
                    cmd.arg("--config");
                    cmd.arg(format!("model_reasoning_effort=\"{reasoning}\""));
                }
                if let Some(ref fmt) = req.output_format
                    && (fmt == "json" || fmt == "stream-json")
                {
                    cmd.arg("--json");
                }

                cmd.arg(&req.prompt);
                cmd
            }
            AgentType::Red | AgentType::LocalLlm => {
                anyhow::bail!("managed spawn currently supports Claude Code and Codex")
            }
        };

        // Run in the project directory
        cmd.current_dir(&req.project);

        // Set env vars for hook correlation
        cmd.env("WISPHIVE_AGENT_ID", &agent_id);
        cmd.env("WISPHIVE_AGENT_TYPE", agent_type.to_string());

        // Managed output is not surfaced yet; avoid child pipe backpressure.
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        let child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn {} — is it installed and on PATH?",
                agent_type
            )
        })?;

        let pid = child.id().context("could not get PID of spawned process")?;

        let managed = ManagedAgent {
            agent_id: agent_id.clone(),
            agent_type,
            pid,
            project: req.project,
            model: req.model,
            name: req.name,
            started_at: Utc::now(),
            reasoning: req.reasoning,
            max_turns: req.max_turns,
            permission_mode: req.permission_mode,
        };

        info!(
            agent_id = %agent_id,
            agent_type = %managed.agent_type,
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
        self.processes.values().map(|p| p.info.clone()).collect()
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
        // Fail-safe: refuse foreign Codex hooks unless explicitly opted in.
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_req(project: &std::path::Path) -> SpawnAgentRequest {
        serde_json::from_value(serde_json::json!({
            "agent_type": "codex",
            "project": project.to_string_lossy(),
            "prompt": "noop",
        }))
        .expect("request should deserialize")
    }

    fn write_codex_hooks(project: &std::path::Path, hooks: serde_json::Value) {
        let dir = project.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            serde_json::to_string(&hooks).unwrap(),
        )
        .unwrap();
    }

    fn rule(command: &str) -> serde_json::Value {
        serde_json::json!({"matcher": "", "hooks": [{"type": "command", "command": command}]})
    }

    /// itr#467: spawning Codex into a project without the Wisphive Codex hook
    /// must fail closed — an ungated agent would bypass the control plane — and
    /// it must do so *before* any process is launched (so no `codex` binary is
    /// required for this to hold).
    #[tokio::test]
    async fn codex_spawn_fails_closed_without_wisphive_hook() {
        let proj = tempfile::tempdir().unwrap();
        let mut registry = ProcessRegistry::new(false);
        let err = registry
            .spawn_agent(codex_req(proj.path()))
            .await
            .expect_err("Codex spawn into an unhooked project must be refused");

        let msg = err.to_string();
        assert!(
            msg.contains("Codex") && msg.contains("hooks install"),
            "refusal should name Codex and the install fix, got: {msg}"
        );
        assert!(
            registry.is_empty(),
            "no process should be tracked after a fail-closed refusal"
        );
    }

    /// itr#471: when the project has the Wisphive hook AND a foreign hook, and
    /// the operator has not opted in, refuse — the trust bypass would run the
    /// foreign hook headlessly. Refusal happens before any spawn.
    #[tokio::test]
    async fn codex_spawn_refuses_foreign_hooks_without_opt_in() {
        let proj = tempfile::tempdir().unwrap();
        write_codex_hooks(
            proj.path(),
            serde_json::json!({"hooks": {"PreToolUse": [
                rule("/opt/bin/wisphive-hook"),
                rule("/usr/bin/third-party-hook.sh"),
            ]}}),
        );

        let mut registry = ProcessRegistry::new(false);
        let err = registry
            .spawn_agent(codex_req(proj.path()))
            .await
            .expect_err("foreign hook + no opt-in must be refused");

        let msg = err.to_string();
        assert!(
            msg.contains("non-Wisphive")
                && msg.contains("third-party-hook.sh")
                && msg.contains("codex_allow_foreign_hooks"),
            "refusal should name the foreign hook and the opt-in, got: {msg}"
        );
        assert!(
            registry.is_empty(),
            "no process should be tracked on refusal"
        );
    }
}
