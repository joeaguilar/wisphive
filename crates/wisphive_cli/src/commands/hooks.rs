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
fn add_hook_entry(settings: &mut serde_json::Value, hook_type: &str, command: &str) {
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
fn remove_hook_entries(settings: &mut serde_json::Value, hook_type: &str, _command: &str) {
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
