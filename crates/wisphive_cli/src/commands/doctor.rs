use std::path::PathBuf;

use anyhow::Result;

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
        issues.push(format!(
            "FAIL  hooks mode is \"off\" (hooks are pass-through)\n      fix: wisphive hooks enable"
        ));
    } else {
        issues.push(format!(
            "FAIL  hooks mode not set (defaults to off)\n      fix: wisphive hooks enable"
        ));
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
                unsafe { libc::kill(pid, 0) == 0 }
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
        issues.push(format!(
            "FAIL  daemon has a stale PID file (process not running)\n      fix: rm ~/.wisphive/wisphive.pid && wisphive daemon start"
        ));
    } else {
        issues.push(format!(
            "FAIL  daemon is not running\n      fix: wisphive daemon start"
        ));
    }

    if daemon_alive && socket_path.exists() {
        eprintln!("  OK  daemon socket exists");
        ok_count += 1;
    } else if daemon_alive && !socket_path.exists() {
        issues.push(format!(
            "FAIL  daemon is running but socket is missing\n      fix: wisphive daemon stop && wisphive daemon start"
        ));
    } else if !daemon_alive && socket_path.exists() {
        issues.push(format!(
            "WARN  stale socket file (daemon not running)\n      fix: rm ~/.wisphive/wisphive.sock && wisphive daemon start"
        ));
    }
    // If both missing, we already reported daemon not running.

    // ── 5. Project hooks ──

    let settings_path = project.join(".claude").join("settings.json");
    if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path).unwrap_or_default();
        if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) {
            let has_hook = settings
                .get("hooks")
                .and_then(|h| h.get("PreToolUse"))
                .and_then(|arr| arr.as_array())
                .is_some_and(|arr| {
                    arr.iter().any(|rule| {
                        // Check nested format
                        rule.get("hooks")
                            .and_then(|h| h.as_array())
                            .is_some_and(|hooks| {
                                hooks.iter().any(|hook| {
                                    hook.get("command")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|cmd| cmd.contains("wisphive"))
                                })
                            })
                            // Check legacy flat format
                            || rule
                                .get("command")
                                .and_then(|v| v.as_str())
                                .is_some_and(|cmd| cmd.contains("wisphive"))
                    })
                });

            if has_hook {
                eprintln!(
                    "  OK  hooks installed in {}",
                    project.file_name().unwrap_or_default().to_string_lossy()
                );
                ok_count += 1;
            } else {
                issues.push(format!(
                    "FAIL  hooks NOT installed in {}\n      fix: wisphive hooks install --project {}",
                    project.file_name().unwrap_or_default().to_string_lossy(),
                    project.display()
                ));
            }
        } else {
            issues.push(format!(
                "FAIL  .claude/settings.json is malformed in {}\n      fix: check the JSON syntax in {}",
                project.file_name().unwrap_or_default().to_string_lossy(),
                settings_path.display()
            ));
        }
    } else {
        issues.push(format!(
            "FAIL  no .claude/settings.json in {}\n      fix: wisphive hooks install --project {}",
            project.file_name().unwrap_or_default().to_string_lossy(),
            project.display()
        ));
    }

    // ── 6. Permissions ──

    if settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) {
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
