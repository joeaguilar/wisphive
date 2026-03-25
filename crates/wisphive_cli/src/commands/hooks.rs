use std::path::PathBuf;

use anyhow::{Context, Result};

/// Permissions that Wisphive adds to .claude/settings.json so Claude Code
/// auto-allows tools that Wisphive will gate via its hook.
/// This eliminates the double-prompt — wisphive becomes the sole gatekeeper.
const WISPHIVE_PERMISSIONS: &[&str] = &["Bash(*)", "Edit(*)", "Write(*)", "NotebookEdit(*)"];

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

/// Install Wisphive hooks into a project's .claude/settings.json.
/// Performs surgical JSON editing — only adds Wisphive entries, preserves everything else.
pub fn install(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    let settings_path = project.join(".claude").join("settings.json");

    // Read existing settings or start fresh
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    let hook_command = hook_binary_path();

    // Ensure hooks object exists
    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }

    // Add PreToolUse hook if not already present
    add_hook_entry(&mut settings, "PreToolUse", &hook_command);

    // Add PostToolUse hook for audit trail (captures tool results)
    add_hook_entry(&mut settings, "PostToolUse", &hook_command);

    // Add PermissionRequest hook for permission management
    add_hook_entry(&mut settings, "PermissionRequest", &hook_command);

    // Add hooks for all other blocking event types
    for event in &[
        "Elicitation",
        "UserPromptSubmit",
        "Stop",
        "SubagentStop",
        "ConfigChange",
        "TeammateIdle",
        "TaskCompleted",
    ] {
        add_hook_entry(&mut settings, event, &hook_command);
    }

    // Add permissions so Claude Code auto-allows tools wisphive gates
    // (eliminates double-prompt — wisphive becomes the sole gatekeeper)
    add_wisphive_permissions(&mut settings);

    // Write back
    let dir = settings_path.parent()
        .ok_or_else(|| anyhow::anyhow!("settings path has no parent directory: {}", settings_path.display()))?;
    std::fs::create_dir_all(dir)?;
    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, formatted)?;

    eprintln!("Wisphive hooks installed in {}", settings_path.display());
    Ok(())
}

/// Remove Wisphive hooks from a project's .claude/settings.json.
/// Only removes entries with the Wisphive hook command — preserves everything else.
pub fn uninstall(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    let settings_path = project.join(".claude").join("settings.json");

    if !settings_path.exists() {
        eprintln!("No .claude/settings.json found in {}", project.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let hook_command = hook_binary_path();

    // Remove Wisphive entries from all hook types
    for event in &[
        "PreToolUse", "PostToolUse", "PermissionRequest",
        "Elicitation", "UserPromptSubmit", "Stop", "SubagentStop",
        "ConfigChange", "TeammateIdle", "TaskCompleted",
    ] {
        remove_hook_entries(&mut settings, event, &hook_command);
    }

    // Remove wisphive-managed permissions (preserves user-added ones)
    remove_wisphive_permissions(&mut settings);

    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, formatted)?;

    eprintln!("Wisphive hooks removed from {}", settings_path.display());
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
        eprintln!("Daemon: running (pid: {})", pid.trim());
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

    Ok(())
}

/// Get the path to the wisphive-hook binary.
fn hook_binary_path() -> String {
    // Look for wisphive-hook next to the wisphive binary, or in PATH
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent() {
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
        && let Some(allow_arr) = permissions.get_mut("allow").and_then(|v| v.as_array_mut()) {
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
    let hooks = settings["hooks"]
        .as_object_mut()
        .expect("hooks should be an object");

    let entries = hooks
        .entry(hook_type)
        .or_insert_with(|| serde_json::json!([]));

    if let Some(arr) = entries.as_array() {
        // Check if our hook is already there (search nested hooks arrays)
        let already_present = arr.iter().any(has_wisphive_hook);
        if already_present {
            return;
        }
    }

    if let Some(arr) = entries.as_array_mut() {
        arr.push(serde_json::json!({
            "matcher": "",
            "hooks": [
                {
                    "type": "command",
                    "command": command
                }
            ]
        }));
    }
}

/// Remove Wisphive hook entries from the settings JSON.
pub(crate) fn remove_hook_entries(
    settings: &mut serde_json::Value,
    hook_type: &str,
    _command: &str,
) {
    if let Some(hooks) = settings.get_mut("hooks")
        && let Some(entries) = hooks.get_mut(hook_type)
            && let Some(arr) = entries.as_array_mut() {
                arr.retain(|rule| !has_wisphive_hook(rule));
            }
}

/// Check if a hook rule entry contains a wisphive hook command.
/// Handles the nested format: {"matcher": "...", "hooks": [{"type": "command", "command": "...wisphive..."}]}
fn has_wisphive_hook(rule: &serde_json::Value) -> bool {
    // Check nested hooks array (correct Claude Code format)
    if let Some(hooks_arr) = rule.get("hooks").and_then(|v| v.as_array()) {
        return hooks_arr.iter().any(|hook| {
            hook.get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("wisphive"))
        });
    }
    // Fallback: check flat format (legacy/simple)
    rule.get("command")
        .and_then(|v| v.as_str())
        .is_some_and(|cmd| cmd.contains("wisphive"))
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
        let bash_count = allow.iter().filter(|v| v.as_str() == Some("Bash(*)")).count();
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
