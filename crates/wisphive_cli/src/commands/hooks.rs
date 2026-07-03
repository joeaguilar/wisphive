use std::path::PathBuf;

use anyhow::{Context, Result};
use wisphive_daemon::project_audit::{
    AgentHookAudit, CLAUDE_HOOK_EVENTS, CODEX_HOOK_EVENTS, HookMode, audit_project,
};

/// Permissions that Wisphive adds to .claude/settings.json so Claude Code
/// auto-allows tools that Wisphive will gate via its hook.
/// This eliminates the double-prompt — wisphive becomes the sole gatekeeper.
const WISPHIVE_PERMISSIONS: &[&str] = &["Bash(*)", "Edit(*)", "Write(*)", "NotebookEdit(*)"];

const CODEX_HOOK_TIMEOUT_SECS: u64 = 3_700;
const CODEX_HOOK_REVIEW_NOTE: &str = "Codex project hooks are non-managed hooks. \
After installing or changing them, open /hooks in Codex and trust the Wisphive hook command; \
project trust alone is not enough for Codex to run them.";

/// Get the wisphive home directory.
fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

/// Get the mode file path.
fn mode_path() -> PathBuf {
    wisphive_home().join("mode")
}

/// Emergency kill switch — writes "off" to the mode file.
pub fn emergency_off() -> Result<()> {
    set_mode("off")?;
    eprintln!("All Wisphive hooks disabled. Run 'wisphive hooks enable' to re-enable.");
    Ok(())
}

/// Set the mode file to the given value.
pub fn set_mode(mode: &str) -> Result<()> {
    let home = wisphive_home();
    std::fs::create_dir_all(&home)?;
    std::fs::write(mode_path(), mode)?;
    eprintln!("Wisphive hooks mode: {mode}");
    Ok(())
}

/// Install Wisphive hooks for Claude Code and Codex.
/// Performs surgical JSON editing — only adds Wisphive entries, preserves everything else.
pub fn install(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    install_claude(&project)?;
    install_codex(&project)?;

    Ok(())
}

fn install_claude(project: &std::path::Path) -> Result<()> {
    let settings_path = project.join(".claude").join("settings.json");

    // Read existing settings or start fresh
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    // Hook commands run via `sh -c`, so a binary path with special characters
    // must be quoted — both for correct execution and so the itr#359 matcher
    // recognizes our own entry on reinstall/uninstall. Plain paths pass
    // through unquoted (no churn in existing settings files).
    let hook_command = shell_quote_command(&hook_binary_path());

    // Ensure hooks object exists
    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }

    for event in CLAUDE_HOOK_EVENTS {
        add_hook_entry(&mut settings, event, &hook_command);
    }

    // Add permissions so Claude Code auto-allows tools wisphive gates
    // (eliminates double-prompt — wisphive becomes the sole gatekeeper)
    add_wisphive_permissions(&mut settings);

    // Write back
    let dir = settings_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "settings path has no parent directory: {}",
            settings_path.display()
        )
    })?;
    std::fs::create_dir_all(dir)?;
    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, formatted)?;

    eprintln!("Wisphive hooks installed in {}", settings_path.display());
    Ok(())
}

fn install_codex(project: &std::path::Path) -> Result<()> {
    let hooks_path = project.join(".codex").join("hooks.json");

    let mut settings: serde_json::Value = if hooks_path.exists() {
        let content = std::fs::read_to_string(&hooks_path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    let hook_command = codex_hook_command(&hook_binary_path());

    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }

    for event in CODEX_HOOK_EVENTS {
        add_hook_entry_with_timeout(
            &mut settings,
            event,
            &hook_command,
            Some(CODEX_HOOK_TIMEOUT_SECS),
        );
    }

    let dir = hooks_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "hooks path has no parent directory: {}",
            hooks_path.display()
        )
    })?;
    std::fs::create_dir_all(dir)?;
    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&hooks_path, formatted)?;

    eprintln!("Wisphive hooks installed in {}", hooks_path.display());
    eprintln!("{CODEX_HOOK_REVIEW_NOTE}");
    Ok(())
}

/// Remove Wisphive hooks for Claude Code and Codex.
/// Only removes entries with the Wisphive hook command — preserves everything else.
pub fn uninstall(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    uninstall_claude(&project)?;
    uninstall_codex(&project)?;

    Ok(())
}

fn uninstall_claude(project: &std::path::Path) -> Result<()> {
    let settings_path = project.join(".claude").join("settings.json");

    if !settings_path.exists() {
        eprintln!("No .claude/settings.json found in {}", project.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let hook_command = hook_binary_path();

    // Remove Wisphive entries from all hook types
    for event in CLAUDE_HOOK_EVENTS {
        remove_hook_entries(&mut settings, event, &hook_command);
    }

    // Remove wisphive-managed permissions (preserves user-added ones)
    remove_wisphive_permissions(&mut settings);

    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, formatted)?;

    eprintln!("Wisphive hooks removed from {}", settings_path.display());
    Ok(())
}

fn uninstall_codex(project: &std::path::Path) -> Result<()> {
    let hooks_path = project.join(".codex").join("hooks.json");

    if !hooks_path.exists() {
        eprintln!("No .codex/hooks.json found in {}", project.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(&hooks_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let hook_command = hook_binary_path();

    for event in CODEX_HOOK_EVENTS {
        remove_hook_entries(&mut settings, event, &hook_command);
    }

    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&hooks_path, formatted)?;

    eprintln!("Wisphive hooks removed from {}", hooks_path.display());
    Ok(())
}

/// Show current hook status.
pub fn status() -> Result<()> {
    // Mode
    let mode = std::fs::read_to_string(mode_path()).unwrap_or_else(|_| "off (not set)".into());
    eprintln!("Mode: {}", mode.trim());

    // Daemon
    let pid_path = wisphive_home().join("wisphive.pid");
    if pid_path.exists() {
        let pid = std::fs::read_to_string(&pid_path)?;
        let pid = pid.trim();
        if let Ok(pid_num) = pid.parse::<i32>() {
            #[cfg(unix)]
            {
                if process_exists(pid_num) {
                    eprintln!("Daemon: running (pid: {pid})");
                } else {
                    eprintln!("Daemon: not running (stale PID file: {pid})");
                }
            }
            #[cfg(not(unix))]
            {
                eprintln!("Daemon PID file: {pid}");
            }
        } else {
            eprintln!("Daemon: invalid PID file ({pid})");
        }
    } else {
        eprintln!("Daemon: not running");
    }

    // Socket
    let socket_path = wisphive_home().join("wisphive.sock");
    if socket_path.exists() {
        eprintln!("Socket: {}", socket_path.display());
    } else {
        eprintln!("Socket: not found");
    }

    if let Ok(project) = std::env::current_dir() {
        let audit = audit_project(&project);
        eprintln!("Project: {}", audit.project_dir.display());
        print_agent_hook_status("Claude Code", &audit.hooks.claude, &audit.hooks.mode);
        print_agent_hook_status("Codex", &audit.hooks.codex, &audit.hooks.mode);

        if audit.hooks.codex.installed {
            eprintln!("Codex: {CODEX_HOOK_REVIEW_NOTE}");
        }
    }

    Ok(())
}

fn print_agent_hook_status(agent_name: &str, audit: &AgentHookAudit, mode: &HookMode) {
    let installed = audit.installed_events.len();
    let total = installed + audit.missing_events.len();

    if audit.enabled {
        eprintln!("{agent_name}: hooks enabled ({installed}/{total} events)");
        return;
    }

    if audit.installed {
        eprintln!(
            "{agent_name}: hooks installed but disabled (mode: {})",
            hook_mode_label(mode)
        );
        return;
    }

    if !audit.config_present {
        eprintln!(
            "{agent_name}: hooks not installed (missing {})",
            audit.config_path.display()
        );
        return;
    }

    if !audit.config_valid {
        let detail = audit
            .read_error
            .as_deref()
            .or(audit.parse_error.as_deref())
            .unwrap_or("invalid hook config");
        eprintln!("{agent_name}: hook config invalid ({detail})");
        return;
    }

    eprintln!("{agent_name}: hooks incomplete ({installed}/{total} events)");
}

fn hook_mode_label(mode: &HookMode) -> String {
    match mode {
        HookMode::Active => "active".into(),
        HookMode::Off => "off".into(),
        HookMode::Missing => "missing".into(),
        HookMode::Invalid(value) => format!("invalid: {value}"),
    }
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Get the path to the wisphive-hook binary.
fn hook_binary_path() -> String {
    // Look for wisphive-hook next to the wisphive binary, or in PATH
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let hook_path = dir.join("wisphive-hook");
        if hook_path.exists() {
            return hook_path.to_string_lossy().to_string();
        }
    }
    // Fallback: assume it's in PATH
    "wisphive-hook".into()
}

/// Add Wisphive-managed permissions to the settings JSON.
/// Ensures Claude Code auto-allows tools that Wisphive gates,
/// eliminating the double-prompt.
fn add_wisphive_permissions(settings: &mut serde_json::Value) {
    if settings.get("permissions").is_none() {
        settings["permissions"] = serde_json::json!({});
    }
    if settings["permissions"].get("allow").is_none() {
        settings["permissions"]["allow"] = serde_json::json!([]);
    }

    if let Some(allow_arr) = settings["permissions"]["allow"].as_array_mut() {
        for &perm in WISPHIVE_PERMISSIONS {
            let already_present = allow_arr
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s == perm));
            if !already_present {
                allow_arr.push(serde_json::Value::String(perm.to_string()));
            }
        }
    }
}

/// Remove Wisphive-managed permissions from the settings JSON.
/// Only removes permissions from the known WISPHIVE_PERMISSIONS list.
fn remove_wisphive_permissions(settings: &mut serde_json::Value) {
    if let Some(permissions) = settings.get_mut("permissions")
        && let Some(allow_arr) = permissions.get_mut("allow").and_then(|v| v.as_array_mut())
    {
        allow_arr.retain(|v| {
            v.as_str()
                .is_some_and(|s| !WISPHIVE_PERMISSIONS.contains(&s))
        });
    }
}

/// Add a Wisphive hook entry to the settings JSON, avoiding duplicates.
///
/// Claude Code hook config format:
/// ```json
/// {
///   "hooks": {
///     "PreToolUse": [
///       {
///         "matcher": "",
///         "hooks": [
///           { "type": "command", "command": "/path/to/wisphive-hook" }
///         ]
///       }
///     ]
///   }
/// }
/// ```
pub(crate) fn add_hook_entry(settings: &mut serde_json::Value, hook_type: &str, command: &str) {
    add_hook_entry_with_timeout(settings, hook_type, command, None);
}

pub(crate) fn add_hook_entry_with_timeout(
    settings: &mut serde_json::Value,
    hook_type: &str,
    command: &str,
    timeout: Option<u64>,
) {
    let hooks = settings["hooks"]
        .as_object_mut()
        .expect("hooks should be an object");

    let entries = hooks
        .entry(hook_type)
        .or_insert_with(|| serde_json::json!([]));

    if let Some(arr) = entries.as_array_mut() {
        let already_present = update_existing_wisphive_hooks(arr, command, timeout);
        if already_present {
            return;
        }
    }

    if let Some(arr) = entries.as_array_mut() {
        let mut command_hook = serde_json::json!({
            "type": "command",
            "command": command
        });
        if let Some(timeout) = timeout
            && let Some(obj) = command_hook.as_object_mut()
        {
            obj.insert("timeout".into(), serde_json::json!(timeout));
        }

        arr.push(serde_json::json!({
            "matcher": "",
            "hooks": [
                command_hook
            ]
        }));
    }
}

fn codex_hook_command(command: &str) -> String {
    format!(
        "env WISPHIVE_AGENT_TYPE=codex {}",
        shell_quote_command(command)
    )
}

fn shell_quote_command(command: &str) -> String {
    if command
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
    {
        return command.to_string();
    }

    format!("'{}'", command.replace('\'', "'\\''"))
}

/// Whether a hook command string invokes the wisphive-hook binary (itr#359).
///
/// Precise on purpose: the old `cmd.contains("wisphive")` matcher rewrote or
/// deleted any USER hook whose command merely mentioned the word — e.g. a
/// project-local script living under a directory named `wisphive`, or a
/// `my-wisphive-logger` wrapper — silently corrupting settings.json on
/// install/uninstall. We parse instead: strip an `env VAR=...` prefix, take
/// argv[0], and compare its basename to `wisphive-hook`.
pub(crate) fn is_wisphive_hook_command(cmd: &str) -> bool {
    // Strip an `env [flags] VAR=... [flags]` prefix: assignments and flags
    // precede the actual command word. `-u`/`--unset` consume a NAME operand.
    let mut rest = cmd.trim_start();
    let (first, after_env) = split_word(rest);
    if first == "env" {
        rest = after_env;
        loop {
            let (word, after) = split_word(rest);
            if word.is_empty() {
                return false;
            }
            if word.starts_with('\'') || word.starts_with('"') {
                // A quoted word is the command, never an assignment — a
                // hook path may itself contain '=' (e.g. .../build=debug/).
                break;
            } else if word == "-u" || word == "--unset" {
                let (_, after_name) = split_word(after);
                rest = after_name;
            } else if word.contains('=') || word.starts_with('-') {
                rest = after;
            } else {
                break;
            }
        }
    }
    let Some(argv0) = parse_argv0(rest) else {
        return false;
    };
    let base = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(&argv0);
    base == "wisphive-hook"
}

/// Pop the next whitespace-delimited word; returns (word, rest-after-word).
fn split_word(s: &str) -> (&str, &str) {
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], s[end..].trim_start())
}

/// Extract argv[0] with shell quoting semantics: concatenated unquoted /
/// single-quoted / double-quoted segments up to the first unquoted
/// whitespace, so the installer's own `'\''` apostrophe escape (see
/// `shell_quote_command`) round-trips. Unterminated quotes yield None.
fn parse_argv0(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            c if c.is_whitespace() => break,
            '\'' => {
                it.next();
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(ch) => out.push(ch),
                        None => return None,
                    }
                }
            }
            '"' => {
                it.next();
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => out.push(it.next()?),
                        Some(ch) => out.push(ch),
                        None => return None,
                    }
                }
            }
            '\\' => {
                it.next();
                out.push(it.next()?);
            }
            _ => {
                out.push(c);
                it.next();
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn update_existing_wisphive_hooks(
    rules: &mut [serde_json::Value],
    command: &str,
    timeout: Option<u64>,
) -> bool {
    let mut found = false;
    for rule in rules {
        if let Some(hooks_arr) = rule.get_mut("hooks").and_then(|v| v.as_array_mut()) {
            for hook in hooks_arr {
                let is_wisphive = hook
                    .get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command);
                if is_wisphive {
                    found = true;
                    update_command_hook(hook, command, timeout);
                }
            }
        } else {
            let is_wisphive = rule
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(is_wisphive_hook_command);
            if is_wisphive {
                found = true;
                update_command_hook(rule, command, timeout);
            }
        }
    }
    found
}

fn update_command_hook(hook: &mut serde_json::Value, command: &str, timeout: Option<u64>) {
    if let Some(obj) = hook.as_object_mut() {
        obj.insert(
            "command".into(),
            serde_json::Value::String(command.to_string()),
        );
        if let Some(timeout) = timeout {
            obj.insert("timeout".into(), serde_json::json!(timeout));
        }
    }
}

/// Remove Wisphive hook entries from the settings JSON.
///
/// Surgical on purpose (itr#359): only OUR command hooks are removed from a
/// rule's nested hooks array; the rule survives if any user hooks remain in
/// it. A rule is dropped only when nothing of the user's is left in it.
pub(crate) fn remove_hook_entries(
    settings: &mut serde_json::Value,
    hook_type: &str,
    _command: &str,
) {
    if let Some(hooks) = settings.get_mut("hooks")
        && let Some(entries) = hooks.get_mut(hook_type)
        && let Some(arr) = entries.as_array_mut()
    {
        arr.retain_mut(|rule| {
            if let Some(hooks_arr) = rule.get_mut("hooks").and_then(|v| v.as_array_mut()) {
                hooks_arr.retain(|hook| {
                    !hook
                        .get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(is_wisphive_hook_command)
                });
                !hooks_arr.is_empty()
            } else {
                !rule
                    .get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command)
            }
        });
    }
}

/// Check if a hook rule entry contains a wisphive hook command.
/// Handles the nested format: {"matcher": "...", "hooks": [{"type": "command",
/// "command": "...wisphive-hook"}]} AND the flat legacy format — both branches
/// are checked (no early return) so a hybrid rule carrying a flat wisphive
/// command next to a user hooks array is still detected.
pub(crate) fn has_wisphive_hook(rule: &serde_json::Value) -> bool {
    let nested = rule
        .get("hooks")
        .and_then(|v| v.as_array())
        .is_some_and(|hooks_arr| {
            hooks_arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command)
            })
        });
    nested
        || rule
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(is_wisphive_hook_command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn temp_project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_settings(project: &std::path::Path, settings: &serde_json::Value) {
        let dir = project.join(".claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(settings).unwrap(),
        )
        .unwrap();
    }

    fn read_settings(project: &std::path::Path) -> serde_json::Value {
        let content = fs::read_to_string(project.join(".claude").join("settings.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn write_codex_hooks(project: &std::path::Path, settings: &serde_json::Value) {
        let dir = project.join(".codex");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("hooks.json"),
            serde_json::to_string_pretty(settings).unwrap(),
        )
        .unwrap();
    }

    fn read_codex_hooks(project: &std::path::Path) -> serde_json::Value {
        let content = fs::read_to_string(project.join(".codex").join("hooks.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    /// Build a Claude Code-format hook rule.
    fn cc_rule(command: &str) -> serde_json::Value {
        json!({"matcher": "", "hooks": [{"type": "command", "command": command}]})
    }

    // ══ add_hook_entry (writes correct nested format) ══

    #[test]
    fn add_to_empty_creates_nested_format() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook");
        let rule = &s["hooks"]["PreToolUse"][0];
        assert_eq!(rule["matcher"], "");
        assert_eq!(rule["hooks"][0]["type"], "command");
        assert_eq!(rule["hooks"][0]["command"], "wisphive-hook");
    }

    #[test]
    fn add_preserves_existing_rules() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("other-hook")]}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hooks"][0]["command"], "other-hook");
        assert_eq!(arr[1]["hooks"][0]["command"], "wisphive-hook");
    }

    #[test]
    fn add_is_idempotent() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut s, "PreToolUse", "/usr/bin/wisphive-hook");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn add_different_hook_types_independent() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut s, "PostToolUse", "wisphive-hook");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn add_with_full_path() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "/home/user/.cargo/bin/wisphive-hook");
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "/home/user/.cargo/bin/wisphive-hook"
        );
    }

    #[test]
    fn add_with_timeout_sets_command_timeout() {
        let mut s = json!({"hooks": {}});
        add_hook_entry_with_timeout(&mut s, "PreToolUse", "wisphive-hook", Some(42));
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 42);
    }

    #[test]
    fn codex_hook_command_sets_agent_type_env() {
        assert_eq!(
            codex_hook_command("/usr/local/bin/wisphive-hook"),
            "env WISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook"
        );
    }

    #[test]
    fn codex_hook_command_quotes_spaces() {
        assert_eq!(
            codex_hook_command("/Applications/Wisphive Tools/wisphive-hook"),
            "env WISPHIVE_AGENT_TYPE=codex '/Applications/Wisphive Tools/wisphive-hook'"
        );
    }

    #[test]
    fn add_updates_existing_wisphive_hook_with_timeout() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("wisphive-hook")]}});
        add_hook_entry_with_timeout(
            &mut s,
            "PreToolUse",
            "env WISPHIVE_AGENT_TYPE=codex wisphive-hook",
            Some(CODEX_HOOK_TIMEOUT_SECS),
        );
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["hooks"][0]["command"],
            "env WISPHIVE_AGENT_TYPE=codex wisphive-hook"
        );
        assert_eq!(arr[0]["hooks"][0]["timeout"], CODEX_HOOK_TIMEOUT_SECS);
    }

    // ══ remove_hook_entries (handles nested + legacy) ══

    #[test]
    fn remove_nested_format() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("other"), cc_rule("wisphive-hook"), cc_rule("another")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hooks"][0]["command"], "other");
        assert_eq!(arr[1]["hooks"][0]["command"], "another");
    }

    #[test]
    fn remove_legacy_flat_format() {
        let mut s = json!({"hooks": {"PreToolUse": [
            {"command": "other"}, {"command": "wisphive-hook"}, {"command": "another"}
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn remove_all_path_variants() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("wisphive-hook"),
            cc_rule("/usr/local/bin/wisphive-hook"),
            cc_rule("/home/u/.cargo/bin/wisphive-hook"),
            cc_rule("other-tool")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "other-tool");
    }

    #[test]
    fn remove_missing_hooks_section() {
        let mut s = json!({"other": "data"});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, json!({"other": "data"}));
    }

    #[test]
    fn remove_missing_hook_type() {
        let mut s = json!({"hooks": {}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, json!({"hooks": {}}));
    }

    #[test]
    fn remove_empty_array() {
        let mut s = json!({"hooks": {"PreToolUse": []}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn remove_noop_when_no_wisphive() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("a"), cc_rule("b")]}});
        let orig = s.clone();
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, orig);
    }

    #[test]
    fn remove_keeps_entries_without_command() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("wisphive-hook"), {"not_command": "x"}, cc_rule("other")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn remove_non_array_graceful() {
        let mut s = json!({"hooks": {"PreToolUse": "not-an-array"}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"], "not-an-array");
    }

    // ══ has_wisphive_hook detection ══

    #[test]
    fn detect_nested() {
        assert!(has_wisphive_hook(&cc_rule("wisphive-hook")));
    }
    #[test]
    fn detect_legacy() {
        assert!(has_wisphive_hook(&json!({"command": "wisphive-hook"})));
    }
    #[test]
    fn detect_false_other() {
        assert!(!has_wisphive_hook(&cc_rule("other-tool")));
    }
    #[test]
    fn detect_false_empty() {
        assert!(!has_wisphive_hook(&json!({})));
    }
    #[test]
    fn detect_path_variants() {
        assert!(has_wisphive_hook(&cc_rule("/usr/local/bin/wisphive-hook")));
        assert!(has_wisphive_hook(&cc_rule("wisphive-hook --verbose")));
    }

    // ══ install / uninstall filesystem integration ══

    #[test]
    fn matcher_recognizes_only_the_wisphive_hook_binary() {
        // itr#359: positives — every form the installer writes.
        for ours in [
            "wisphive-hook",
            "/Users/x/.cargo/bin/wisphive-hook",
            "env WISPHIVE_AGENT_TYPE=codex /Users/x/.cargo/bin/wisphive-hook",
            "env WISPHIVE_AGENT_TYPE=codex '/Users/x/my tools/wisphive-hook'",
            // installer's own '\'' apostrophe escape must round-trip
            r"env WISPHIVE_AGENT_TYPE=codex '/Users/x/Joe'\''s tools/wisphive-hook'",
            "\"/Users/x/my tools/wisphive-hook\"",
            // hand-edited but shell-valid variants
            "env\tWISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook",
            "env -u FOO WISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook",
            r"/Users/x/my\ tools/wisphive-hook",
        ] {
            assert!(is_wisphive_hook_command(ours), "should match: {ours}");
        }
        // Negatives — user hooks that merely mention the word.
        for theirs in [
            "/Users/x/AI_Projects/wisphive/scripts/precommit.sh",
            "my-wisphive-logger --verbose",
            "python3 /opt/wisphive-tools/check.py",
            "wisphive doctor",
            "'/unterminated/quote/wisphive-hook",
            "env FOO=bar",
            "",
        ] {
            assert!(
                !is_wisphive_hook_command(theirs),
                "must NOT match: {theirs}"
            );
        }
    }

    #[test]
    fn matcher_round_trips_installer_quoting() {
        // Whatever shell_quote_command emits for the hook path — plain,
        // spaced, or apostrophed — both install forms must be recognized.
        for path in [
            "/Users/x/.cargo/bin/wisphive-hook",
            "/Users/x/my tools/wisphive-hook",
            "/Users/x/Joe's tools/wisphive-hook",
            "/Users/x/build=debug/wisphive-hook",
        ] {
            let claude_form = shell_quote_command(path);
            let codex_form = codex_hook_command(path);
            assert!(
                is_wisphive_hook_command(&claude_form),
                "claude form should match: {claude_form}"
            );
            assert!(
                is_wisphive_hook_command(&codex_form),
                "codex form should match: {codex_form}"
            );
        }
    }

    #[test]
    fn has_wisphive_hook_checks_both_nested_and_flat() {
        // Hybrid rule: flat wisphive command next to a user-only hooks array
        // must still be detected (no early return on the nested branch).
        let hybrid = json!({
            "command": "/usr/local/bin/wisphive-hook",
            "hooks": [{"type": "command", "command": "user-lint.sh"}]
        });
        assert!(has_wisphive_hook(&hybrid));
        let user_only = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "user-lint.sh"}]
        });
        assert!(!has_wisphive_hook(&user_only));
    }

    #[test]
    fn uninstall_keeps_user_hook_sharing_a_rule_with_ours() {
        // itr#359 destructive class: a user hook grouped into the SAME rule's
        // hooks array as wisphive-hook must survive uninstall.
        let mut s = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "*",
                    "hooks": [
                        {"type": "command", "command": "/usr/local/bin/wisphive-hook"},
                        {"type": "command", "command": "/Users/x/bin/my-lint.sh"}
                    ]
                }]
            }
        });
        remove_hook_entries(&mut s, "PreToolUse", "unused");
        let rules = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(
            rules.len(),
            1,
            "rule with a surviving user hook was dropped"
        );
        let cmds: Vec<&str> = rules[0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert_eq!(cmds, vec!["/Users/x/bin/my-lint.sh"]);
    }

    #[test]
    fn install_and_uninstall_leave_user_wisphive_named_hooks_alone() {
        // itr#359 acceptance: a user hook whose command contains "wisphive"
        // but is not the wisphive-hook binary survives install AND uninstall.
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        let user_cmd = "/Users/x/AI_Projects/wisphive/scripts/lint.sh";
        write_settings(
            &p,
            &json!({
                "hooks": {
                    "PreToolUse": [
                        {"matcher": "*", "hooks": [{"type": "command", "command": user_cmd}]}
                    ]
                }
            }),
        );

        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let cmds: Vec<String> = s["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect();
        assert!(
            cmds.iter().any(|c| c == user_cmd),
            "user hook rewritten by install: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| is_wisphive_hook_command(c)),
            "wisphive hook not installed: {cmds:?}"
        );

        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let cmds: Vec<String> = s["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect();
        assert!(
            cmds.iter().any(|c| c == user_cmd),
            "user hook deleted by uninstall: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|c| is_wisphive_hook_command(c)),
            "wisphive hook not removed: {cmds:?}"
        );
    }

    #[test]
    fn install_creates_from_scratch() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let rule = &s["hooks"]["PreToolUse"][0];
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("wisphive")
        );
        assert_eq!(rule["hooks"][0]["type"], "command");
    }

    #[test]
    fn install_creates_codex_hooks_with_timeout() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        let rule = &s["hooks"]["PreToolUse"][0];
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("wisphive")
        );
        assert_eq!(rule["hooks"][0]["type"], "command");
        assert_eq!(rule["hooks"][0]["timeout"], CODEX_HOOK_TIMEOUT_SECS);
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("WISPHIVE_AGENT_TYPE=codex")
        );
        assert!(s.get("permissions").is_none());
    }

    #[test]
    fn install_preserves_existing_settings() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"mcpServers": {"s": {"url": "http://x"}}}));
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["mcpServers"]["s"]["url"], "http://x");
        assert!(s["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn install_preserves_existing_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("linter")],
                "PostToolUse": [cc_rule("logger")]
            }}),
        );
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn install_preserves_existing_codex_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_codex_hooks(&p, &json!({"hooks": {"PreToolUse": [cc_rule("policy")]}}));
        install(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "policy");
    }

    #[test]
    fn install_idempotent() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        install(Some(p.clone()), false).unwrap();
        install(Some(p.clone()), false).unwrap();
        assert_eq!(
            read_settings(&p)["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn uninstall_removes_wisphive_only() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("linter"), cc_rule("wisphive-hook")],
                "PostToolUse": [cc_rule("wisphive-hook"), cc_rule("logger")]
            }, "mcpServers": {"keep": "this"}}),
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["mcpServers"]["keep"], "this");
    }

    #[test]
    fn uninstall_removes_codex_wisphive_only() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_codex_hooks(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("policy"), cc_rule("wisphive-hook")],
                "PostToolUse": [cc_rule("wisphive-hook"), cc_rule("logger")]
            }}),
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "policy");
        assert_eq!(
            s["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "logger"
        );
    }

    #[test]
    fn uninstall_ok_no_settings() {
        let tmp = temp_project();
        assert!(uninstall(Some(tmp.path().to_path_buf()), false).is_ok());
    }

    #[test]
    fn install_then_uninstall_round_trip() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {"PreToolUse": [cc_rule("existing")]}, "other": "data"}),
        );
        install(Some(p.clone()), false).unwrap();
        assert_eq!(
            read_settings(&p)["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "existing"
        );
        assert_eq!(s["other"], "data");
    }

    #[test]
    fn install_no_hooks_key() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"someKey": "someValue"}));
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["someKey"], "someValue");
        assert!(s["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn uninstall_empty_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"hooks": {}}));
        assert!(uninstall(Some(p), false).is_ok());
    }

    // ══ Mode file ══

    // ══ Permissions management ══

    #[test]
    fn add_permissions_to_empty_settings() {
        let mut s = json!({});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "Bash(*)"));
        assert!(allow.iter().any(|v| v == "Edit(*)"));
        assert!(allow.iter().any(|v| v == "Write(*)"));
        assert!(allow.iter().any(|v| v == "NotebookEdit(*)"));
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn add_permissions_preserves_existing() {
        let mut s = json!({"permissions": {"allow": ["mcp__foo(*)"]}});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "mcp__foo(*)"));
        assert!(allow.iter().any(|v| v == "Bash(*)"));
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len() + 1);
    }

    #[test]
    fn add_permissions_idempotent() {
        let mut s = json!({});
        add_wisphive_permissions(&mut s);
        add_wisphive_permissions(&mut s);
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn add_permissions_no_duplicates_when_user_has_same() {
        let mut s = json!({"permissions": {"allow": ["Bash(*)"]}});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        let bash_count = allow
            .iter()
            .filter(|v| v.as_str() == Some("Bash(*)"))
            .count();
        assert_eq!(bash_count, 1);
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn remove_permissions_cleans_wisphive_only() {
        let mut s = json!({"permissions": {"allow": [
            "Bash(*)", "Edit(*)", "Write(*)", "NotebookEdit(*)", "mcp__custom(*)"
        ]}});
        remove_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0], "mcp__custom(*)");
    }

    #[test]
    fn remove_permissions_noop_when_empty() {
        let mut s = json!({});
        remove_wisphive_permissions(&mut s);
        assert!(s.get("permissions").is_none());
    }

    #[test]
    fn remove_permissions_noop_when_no_allow() {
        let mut s = json!({"permissions": {"deny": ["something"]}});
        remove_wisphive_permissions(&mut s);
        assert!(s["permissions"].get("allow").is_none());
        assert_eq!(s["permissions"]["deny"][0], "something");
    }

    #[test]
    fn install_adds_both_hooks_and_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert!(s["hooks"]["PreToolUse"].is_array());
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "Bash(*)"));
    }

    #[test]
    fn uninstall_removes_both_hooks_and_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let hooks = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(hooks.is_empty());
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.is_empty());
    }

    #[test]
    fn round_trip_preserves_user_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({
                "permissions": {"allow": ["mcp__github(*)"]},
                "hooks": {"PreToolUse": [cc_rule("linter")]}
            }),
        );
        install(Some(p.clone()), false).unwrap();
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0], "mcp__github(*)");
        let hooks = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hooks"][0]["command"], "linter");
    }

    // ══ Mode file ══

    #[test]
    fn mode_creates_and_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("mode");
        std::fs::write(&f, "active").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "active");
        std::fs::write(&f, "off").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "off");
    }

    #[test]
    fn mode_missing_defaults_off() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("nope").join("mode");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap_or_else(|_| "off".into()),
            "off"
        );
    }

    #[test]
    fn mode_whitespace_trimmed() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("mode");
        std::fs::write(&f, "  active  \n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap().trim(), "active");
    }
}
