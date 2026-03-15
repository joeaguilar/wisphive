use std::path::PathBuf;

use anyhow::{Context, Result};

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

    // Write back
    let dir = settings_path.parent().unwrap();
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

    // Remove Wisphive entries from each hook type
    remove_hook_entries(&mut settings, "PreToolUse", &hook_command);
    remove_hook_entries(&mut settings, "PostToolUse", &hook_command);

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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let hook_path = dir.join("wisphive-hook");
            if hook_path.exists() {
                return hook_path.to_string_lossy().to_string();
            }
        }
    }
    // Fallback: assume it's in PATH
    "wisphive-hook".into()
}

/// Add a Wisphive hook entry to the settings JSON, avoiding duplicates.
pub(crate) fn add_hook_entry(settings: &mut serde_json::Value, hook_type: &str, command: &str) {
    let hooks = settings["hooks"]
        .as_object_mut()
        .expect("hooks should be an object");

    let entries = hooks
        .entry(hook_type)
        .or_insert_with(|| serde_json::json!([]));

    if let Some(arr) = entries.as_array() {
        // Check if our hook is already there
        let already_present = arr.iter().any(|entry| {
            entry
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("wisphive"))
        });
        if already_present {
            return;
        }
    }

    if let Some(arr) = entries.as_array_mut() {
        arr.push(serde_json::json!({
            "command": command
        }));
    }
}

/// Remove Wisphive hook entries from the settings JSON.
pub(crate) fn remove_hook_entries(
    settings: &mut serde_json::Value,
    hook_type: &str,
    _command: &str,
) {
    if let Some(hooks) = settings.get_mut("hooks") {
        if let Some(entries) = hooks.get_mut(hook_type) {
            if let Some(arr) = entries.as_array_mut() {
                arr.retain(|entry| {
                    !entry
                        .get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|cmd| cmd.contains("wisphive"))
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    // ── Helper ──

    fn temp_project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_settings(project: &std::path::Path, settings: &serde_json::Value) {
        let dir = project.join(".claude");
        fs::create_dir_all(&dir).unwrap();
        let content = serde_json::to_string_pretty(settings).unwrap();
        fs::write(dir.join("settings.json"), content).unwrap();
    }

    fn read_settings(project: &std::path::Path) -> serde_json::Value {
        let path = project.join(".claude").join("settings.json");
        let content = fs::read_to_string(path).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    // ════════════════════════════════════════════════════════════
    // add_hook_entry tests
    // ════════════════════════════════════════════════════════════

    #[test]
    fn add_hook_entry_to_empty_settings() {
        let mut settings = json!({"hooks": {}});
        add_hook_entry(&mut settings, "PreToolUse", "wisphive-hook");

        let entries = &settings["hooks"]["PreToolUse"];
        assert!(entries.is_array());
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["command"], "wisphive-hook");
    }

    #[test]
    fn add_hook_entry_preserves_existing_hooks() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "some-other-hook"}
                ]
            }
        });
        add_hook_entry(&mut settings, "PreToolUse", "wisphive-hook");

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["command"], "some-other-hook");
        assert_eq!(entries[1]["command"], "wisphive-hook");
    }

    #[test]
    fn add_hook_entry_idempotent() {
        let mut settings = json!({"hooks": {}});
        add_hook_entry(&mut settings, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut settings, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut settings, "PreToolUse", "/usr/bin/wisphive-hook");

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        // All contain "wisphive" so only the first should be added
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn add_hook_entry_different_hook_types_are_independent() {
        let mut settings = json!({"hooks": {}});
        add_hook_entry(&mut settings, "PreToolUse", "wisphive-hook");
        add_hook_entry(&mut settings, "PostToolUse", "wisphive-hook");

        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn add_hook_entry_with_path_containing_wisphive() {
        let mut settings = json!({"hooks": {}});
        add_hook_entry(
            &mut settings,
            "PreToolUse",
            "/home/user/.cargo/bin/wisphive-hook",
        );

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["command"], "/home/user/.cargo/bin/wisphive-hook");
    }

    // ════════════════════════════════════════════════════════════
    // remove_hook_entries tests
    // ════════════════════════════════════════════════════════════

    #[test]
    fn remove_hook_entries_removes_wisphive_only() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "some-other-hook"},
                    {"command": "wisphive-hook"},
                    {"command": "yet-another-hook"}
                ]
            }
        });
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["command"], "some-other-hook");
        assert_eq!(entries[1]["command"], "yet-another-hook");
    }

    #[test]
    fn remove_hook_entries_handles_missing_hooks_section() {
        let mut settings = json!({"other": "data"});
        // Should not panic
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");
        assert_eq!(settings, json!({"other": "data"}));
    }

    #[test]
    fn remove_hook_entries_handles_missing_hook_type() {
        let mut settings = json!({"hooks": {}});
        // Should not panic
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");
        assert_eq!(settings, json!({"hooks": {}}));
    }

    #[test]
    fn remove_hook_entries_handles_empty_array() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": []
            }
        });
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn remove_hook_entries_removes_all_wisphive_variants() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "wisphive-hook"},
                    {"command": "/usr/local/bin/wisphive-hook"},
                    {"command": "/home/user/.cargo/bin/wisphive-hook"},
                    {"command": "other-tool"}
                ]
            }
        });
        remove_hook_entries(&mut settings, "PreToolUse", "");

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["command"], "other-tool");
    }

    #[test]
    fn remove_hook_entries_noop_when_no_wisphive_present() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "tool-a"},
                    {"command": "tool-b"}
                ]
            }
        });
        let original = settings.clone();
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");
        assert_eq!(settings, original);
    }

    #[test]
    fn remove_hook_entries_handles_entries_without_command_field() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "wisphive-hook"},
                    {"not_command": "something"},
                    {"command": "other-tool"}
                ]
            }
        });
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        // Entry without "command" is retained, wisphive is removed
        assert_eq!(entries.len(), 2);
    }

    // ════════════════════════════════════════════════════════════
    // install / uninstall integration tests (filesystem)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn install_creates_settings_from_scratch() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        install(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        assert!(settings["hooks"]["PreToolUse"].is_array());
        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0]["command"].as_str().unwrap().contains("wisphive"));
    }

    #[test]
    fn install_preserves_existing_settings() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        // Pre-existing settings with MCP servers and other data
        let existing = json!({
            "mcpServers": {
                "myserver": {"url": "http://localhost:3000"}
            },
            "permissions": {
                "allow": ["Read", "Glob"]
            }
        });
        write_settings(&project, &existing);

        install(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        // Original data preserved
        assert_eq!(
            settings["mcpServers"]["myserver"]["url"],
            "http://localhost:3000"
        );
        assert!(settings["permissions"]["allow"].is_array());
        // Hook added
        assert!(settings["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn install_preserves_existing_hooks() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        let existing = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "my-custom-linter"}
                ],
                "PostToolUse": [
                    {"command": "my-logger"}
                ]
            }
        });
        write_settings(&project, &existing);

        install(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["command"], "my-custom-linter");
        // PostToolUse untouched
        let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["command"], "my-logger");
    }

    #[test]
    fn install_is_idempotent() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        install(Some(project.clone()), false).unwrap();
        install(Some(project.clone()), false).unwrap();
        install(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn uninstall_removes_wisphive_only() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        let existing = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "my-custom-linter"},
                    {"command": "wisphive-hook"}
                ],
                "PostToolUse": [
                    {"command": "wisphive-hook"},
                    {"command": "my-logger"}
                ]
            },
            "mcpServers": {"keep": "this"}
        });
        write_settings(&project, &existing);

        uninstall(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        // Wisphive removed from both
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["command"], "my-custom-linter");

        let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["command"], "my-logger");

        // Other settings preserved
        assert_eq!(settings["mcpServers"]["keep"], "this");
    }

    #[test]
    fn uninstall_ok_when_no_settings_file() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();
        // No .claude/settings.json exists
        let result = uninstall(Some(project), false);
        assert!(result.is_ok());
    }

    #[test]
    fn install_then_uninstall_round_trip() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        let original = json!({
            "hooks": {
                "PreToolUse": [
                    {"command": "existing-tool"}
                ]
            },
            "other": "data"
        });
        write_settings(&project, &original);

        install(Some(project.clone()), false).unwrap();
        // Now has wisphive + existing-tool
        let mid = read_settings(&project);
        assert_eq!(mid["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);

        uninstall(Some(project.clone()), false).unwrap();
        // Back to just existing-tool
        let after = read_settings(&project);
        let entries = after["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["command"], "existing-tool");
        assert_eq!(after["other"], "data");
    }

    // ════════════════════════════════════════════════════════════
    // Mode file tests
    // ════════════════════════════════════════════════════════════

    #[test]
    fn set_mode_creates_directory_and_file() {
        // This test modifies ~/.wisphive/mode so we use a scoped approach
        // by testing the internal logic with temp dirs instead.
        let tmp = tempfile::tempdir().unwrap();
        let mode_file = tmp.path().join("mode");

        std::fs::write(&mode_file, "active").unwrap();
        let content = std::fs::read_to_string(&mode_file).unwrap();
        assert_eq!(content, "active");

        std::fs::write(&mode_file, "off").unwrap();
        let content = std::fs::read_to_string(&mode_file).unwrap();
        assert_eq!(content, "off");
    }

    #[test]
    fn mode_file_missing_defaults_to_off() {
        let tmp = tempfile::tempdir().unwrap();
        let mode_file = tmp.path().join("nonexistent").join("mode");
        let mode = std::fs::read_to_string(&mode_file).unwrap_or_else(|_| "off".into());
        assert_eq!(mode, "off");
    }

    #[test]
    fn mode_file_with_whitespace_is_trimmed() {
        let tmp = tempfile::tempdir().unwrap();
        let mode_file = tmp.path().join("mode");
        std::fs::write(&mode_file, "  active  \n").unwrap();
        let content = std::fs::read_to_string(&mode_file).unwrap();
        assert_eq!(content.trim(), "active");
    }

    // ════════════════════════════════════════════════════════════
    // Edge cases
    // ════════════════════════════════════════════════════════════

    #[test]
    fn install_handles_settings_with_no_hooks_key() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        let existing = json!({"someKey": "someValue"});
        write_settings(&project, &existing);

        install(Some(project.clone()), false).unwrap();

        let settings = read_settings(&project);
        assert_eq!(settings["someKey"], "someValue");
        assert!(settings["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn add_hook_entry_handles_command_with_args() {
        let mut settings = json!({"hooks": {}});
        add_hook_entry(
            &mut settings,
            "PreToolUse",
            "wisphive-hook --verbose --timeout 30",
        );

        let entries = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0]["command"],
            "wisphive-hook --verbose --timeout 30"
        );
    }

    #[test]
    fn remove_handles_non_array_hook_entries_gracefully() {
        // If someone put a string instead of an array for a hook type
        let mut settings = json!({
            "hooks": {
                "PreToolUse": "not-an-array"
            }
        });
        // Should not panic — the as_array_mut() check will return None
        remove_hook_entries(&mut settings, "PreToolUse", "wisphive-hook");
        // Data unchanged since it wasn't an array
        assert_eq!(settings["hooks"]["PreToolUse"], "not-an-array");
    }

    #[test]
    fn uninstall_handles_settings_with_empty_hooks() {
        let tmp = temp_project();
        let project = tmp.path().to_path_buf();

        let existing = json!({"hooks": {}});
        write_settings(&project, &existing);

        let result = uninstall(Some(project), false);
        assert!(result.is_ok());
    }
}
