use std::path::PathBuf;

use anyhow::Result;
use wisphive_daemon::project_audit::{AgentHookAudit, audit_project};

const CODEX_HOOK_REVIEW_NOTE: &str = "Codex project hooks require /hooks review inside Codex; \
trust the Wisphive hook command there if Codex does not appear in Wisphive after a tool call.";

pub fn run(project: Option<PathBuf>) -> Result<()> {
    let home = wisphive_home();
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut issues: Vec<String> = Vec::new();
    let mut ok_count = 0;

    // ── 1. Binaries ──

    check(
        "wisphive binary",
        which("wisphive"),
        "cargo build --release && cp target/release/wisphive ~/.cargo/bin/",
        &mut issues,
        &mut ok_count,
    );

    check(
        "wisphive-hook binary",
        which("wisphive-hook"),
        "cargo build --release && cp target/release/wisphive-hook ~/.cargo/bin/",
        &mut issues,
        &mut ok_count,
    );

    // ── 2. Home directory ──

    check(
        "~/.wisphive directory",
        home.is_dir(),
        "mkdir -p ~/.wisphive",
        &mut issues,
        &mut ok_count,
    );

    // ── 3. Mode file ──

    let mode = std::fs::read_to_string(home.join("mode"))
        .unwrap_or_default()
        .trim()
        .to_string();

    if mode == "active" {
        eprintln!("  OK  hooks mode is active");
        ok_count += 1;
    } else if mode == "off" {
        issues.push("FAIL  hooks mode is \"off\" (hooks are pass-through)\n      fix: wisphive hooks enable".to_string());
    } else {
        issues.push(
            "FAIL  hooks mode not set (defaults to off)\n      fix: wisphive hooks enable"
                .to_string(),
        );
    }

    // ── 4. Daemon ──

    let pid_path = home.join("wisphive.pid");
    let socket_path = home.join("wisphive.sock");
    let daemon_alive = if pid_path.exists() {
        let pid_str = std::fs::read_to_string(&pid_path).unwrap_or_default();
        let pid: i32 = pid_str.trim().parse().unwrap_or(0);
        if pid > 0 {
            #[cfg(unix)]
            {
                process_exists(pid)
            }
            #[cfg(not(unix))]
            {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if daemon_alive {
        eprintln!("  OK  daemon is running");
        ok_count += 1;
    } else if pid_path.exists() {
        issues.push("FAIL  daemon has a stale PID file (process not running)\n      fix: rm ~/.wisphive/wisphive.pid && wisphive daemon start".to_string());
    } else {
        issues.push("FAIL  daemon is not running\n      fix: wisphive daemon start".to_string());
    }

    if daemon_alive && socket_path.exists() {
        eprintln!("  OK  daemon socket exists");
        ok_count += 1;
    } else if daemon_alive && !socket_path.exists() {
        issues.push("FAIL  daemon is running but socket is missing\n      fix: wisphive daemon stop && wisphive daemon start".to_string());
    } else if !daemon_alive && socket_path.exists() {
        issues.push("WARN  stale socket file (daemon not running)\n      fix: rm ~/.wisphive/wisphive.sock && wisphive daemon start".to_string());
    }
    // If both missing, we already reported daemon not running.

    // ── 5. Project hooks ──

    let claude_settings_path = project.join(".claude").join("settings.json");
    let hook_audit = audit_project(&project);
    check_project_hook(
        "Claude Code",
        &hook_audit.hooks.claude,
        &project,
        &mut issues,
        &mut ok_count,
    );

    check_project_hook(
        "Codex",
        &hook_audit.hooks.codex,
        &project,
        &mut issues,
        &mut ok_count,
    );

    // ── 6. Permissions ──

    if claude_settings_path.exists()
        && let Ok(content) = std::fs::read_to_string(&claude_settings_path)
        && let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let has_perms = settings
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("Bash(*)")));

        if has_perms {
            eprintln!("  OK  Claude Code permissions set (no double-prompt)");
            ok_count += 1;
        } else {
            issues.push(format!(
                        "WARN  Claude Code permissions not set (may cause double-prompt)\n      fix: wisphive hooks install --project {}",
                        project.display()
                    ));
        }
    }

    // ── Summary ──

    eprintln!();
    if issues.is_empty() {
        eprintln!("All checks passed ({ok_count}/{ok_count}). Wisphive is ready.");
    } else {
        for issue in &issues {
            eprintln!("  {issue}");
        }
        eprintln!();
        eprintln!("{} passed, {} issue(s) found.", ok_count, issues.len());
    }

    Ok(())
}

fn check(name: &str, ok: bool, fix: &str, issues: &mut Vec<String>, ok_count: &mut usize) {
    if ok {
        eprintln!("  OK  {name}");
        *ok_count += 1;
    } else {
        issues.push(format!("FAIL  {name}\n      fix: {fix}"));
    }
}

fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn check_project_hook(
    agent_name: &str,
    audit: &AgentHookAudit,
    project: &std::path::Path,
    issues: &mut Vec<String>,
    ok_count: &mut usize,
) {
    if !audit.config_present {
        issues.push(format!(
            "FAIL  no {} in {}\n      fix: wisphive hooks install --project {}",
            audit.config_path.display(),
            project.file_name().unwrap_or_default().to_string_lossy(),
            project.display()
        ));
        return;
    }

    if !audit.config_valid {
        let (problem, fix) = if let Some(error) = audit.read_error.as_deref() {
            (
                format!("could not be read ({error})"),
                format!("check access to {}", audit.config_path.display()),
            )
        } else {
            (
                "is malformed JSON".to_string(),
                format!("check the JSON syntax in {}", audit.config_path.display()),
            )
        };
        issues.push(format!(
            "FAIL  {} {problem}\n      fix: {fix}",
            audit.config_path.display()
        ));
        return;
    }

    let gated = audit
        .installed_events
        .iter()
        .any(|event| event == "PreToolUse");
    if audit.installed {
        let total = audit.installed_events.len();
        eprintln!("  OK  {agent_name} hooks fully installed ({total}/{total} events)");
        if agent_name == "Codex" {
            eprintln!("      note: {CODEX_HOOK_REVIEW_NOTE}");
        }
        *ok_count += 1;
        return;
    }

    if gated && agent_name == "Codex" {
        eprintln!("      note: {CODEX_HOOK_REVIEW_NOTE}");
    }

    let status = if gated {
        "WARN  hooks are gated but incomplete"
    } else {
        "FAIL  PreToolUse gate is missing and hooks are incomplete"
    };
    issues.push(format!(
        "{status} for {agent_name}\n      missing expected events: {}\n      fix: wisphive hooks install --project {}",
        audit.missing_events.join(", "),
        project.display()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
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

    fn collect_hook_check(
        agent_name: &str,
        audit: &AgentHookAudit,
        project: &Path,
    ) -> (Vec<String>, usize) {
        let mut issues = Vec::new();
        let mut ok_count = 0;
        check_project_hook(agent_name, audit, project, &mut issues, &mut ok_count);
        (issues, ok_count)
    }

    #[test]
    fn pretool_only_configs_name_every_missing_event_for_both_agents() {
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

        let audit = audit_project(project.path());
        for (name, agent, expected) in [
            ("Claude Code", &audit.hooks.claude, CLAUDE_HOOK_EVENTS),
            ("Codex", &audit.hooks.codex, CODEX_HOOK_EVENTS),
        ] {
            let (issues, ok_count) = collect_hook_check(name, agent, project.path());
            assert_eq!(ok_count, 0);
            assert_eq!(issues.len(), 1);
            let issue = &issues[0];
            assert!(issue.contains("gated but incomplete"));
            assert!(issue.contains("wisphive hooks install --project"));
            for event in expected
                .iter()
                .copied()
                .filter(|event| *event != "PreToolUse")
            {
                assert!(issue.contains(event), "missing {event} in: {issue}");
            }
        }
    }

    #[test]
    fn full_installs_are_reported_healthy_for_both_agents() {
        let project = tempfile::tempdir().unwrap();
        install_hooks(project.path()).unwrap();
        let audit = audit_project(project.path());

        let (claude_issues, claude_ok) =
            collect_hook_check("Claude Code", &audit.hooks.claude, project.path());
        let (codex_issues, codex_ok) =
            collect_hook_check("Codex", &audit.hooks.codex, project.path());

        assert!(claude_issues.is_empty());
        assert!(codex_issues.is_empty());
        assert_eq!(claude_ok, 1);
        assert_eq!(codex_ok, 1);
    }

    #[test]
    fn absent_and_malformed_configs_are_actionable() {
        let project = tempfile::tempdir().unwrap();
        let missing = audit_project(project.path());
        for (name, agent) in [
            ("Claude Code", &missing.hooks.claude),
            ("Codex", &missing.hooks.codex),
        ] {
            let (issues, ok_count) = collect_hook_check(name, agent, project.path());
            assert_eq!(ok_count, 0);
            assert!(issues[0].contains("wisphive hooks install --project"));
            assert!(issues[0].contains(&agent.config_path.display().to_string()));
        }

        fs::create_dir_all(project.path().join(".claude")).unwrap();
        fs::create_dir_all(project.path().join(".codex")).unwrap();
        fs::write(project.path().join(".claude/settings.json"), "{").unwrap();
        fs::write(project.path().join(".codex/hooks.json"), "{").unwrap();
        let malformed = audit_project(project.path());
        for (name, agent) in [
            ("Claude Code", &malformed.hooks.claude),
            ("Codex", &malformed.hooks.codex),
        ] {
            let (issues, ok_count) = collect_hook_check(name, agent, project.path());
            assert_eq!(ok_count, 0);
            assert!(issues[0].contains("malformed JSON"));
            assert!(issues[0].contains("check the JSON syntax"));
        }
    }
}
