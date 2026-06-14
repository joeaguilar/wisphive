use anyhow::{Context, Result};
use std::path::PathBuf;
use wisphive_daemon::UserConfig;

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive").join("config.json")
}

fn load() -> UserConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

fn save(config: &UserConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, json).context("failed to write config.json")?;
    Ok(())
}

pub fn get(key: &str) -> Result<()> {
    let config = load();
    match key {
        "notifications" => eprintln!("{}", config.notifications),
        "hook_timeout_secs" => {
            eprintln!("{}", config.hook_timeout_secs.unwrap_or(3600))
        }
        "agent_timeout_secs" => {
            eprintln!("{}", config.agent_timeout_secs.unwrap_or(300))
        }
        "auto_approve_level" => {
            let level = config.auto_approve_level.unwrap_or_default();
            eprintln!("{level}");
        }
        _ => eprintln!(
            "unknown config key: {key}. Valid: notifications, hook_timeout_secs, agent_timeout_secs, auto_approve_level"
        ),
    }
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let mut config = load();
    match key {
        "notifications" => {
            config.notifications = match value {
                "true" | "1" | "on" | "yes" => true,
                "false" | "0" | "off" | "no" => false,
                _ => anyhow::bail!("invalid value for notifications: {value} (use true/false)"),
            };
        }
        "hook_timeout_secs" => {
            let secs: u64 = value
                .parse()
                .context("hook_timeout_secs must be a number")?;
            config.hook_timeout_secs = Some(secs);
        }
        "agent_timeout_secs" => {
            let secs: u64 = value
                .parse()
                .context("agent_timeout_secs must be a number")?;
            config.agent_timeout_secs = Some(secs);
        }
        "auto_approve_level" => {
            let level: wisphive_protocol::AutoApproveLevel =
                value.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            config.auto_approve_level = Some(level);
        }
        _ => anyhow::bail!(
            "unknown config key: {key}. Valid: notifications, hook_timeout_secs, agent_timeout_secs, auto_approve_level"
        ),
    }
    save(&config)?;
    eprintln!("{key} = {value}");
    eprintln!("Note: restart the daemon for changes to take effect.");
    Ok(())
}

pub fn list() -> Result<()> {
    let config = load();
    eprintln!("notifications = {}", config.notifications);
    eprintln!(
        "hook_timeout_secs = {}",
        config.hook_timeout_secs.unwrap_or(3600)
    );
    eprintln!(
        "agent_timeout_secs = {}",
        config.agent_timeout_secs.unwrap_or(300)
    );
    let level = config.auto_approve_level.unwrap_or_default();
    eprintln!("auto_approve_level = {level}");
    if let Some(ref add) = config.auto_approve_add
        && !add.is_empty()
    {
        eprintln!("auto_approve_add = {}", add.join(", "));
    }
    if let Some(ref remove) = config.auto_approve_remove
        && !remove.is_empty()
    {
        eprintln!("auto_approve_remove = {}", remove.join(", "));
    }
    eprintln!("auto_approve_dangerous = {}", config.auto_approve_dangerous);
    if let Some(ref add) = config.always_ask
        && !add.is_empty()
    {
        eprintln!("always_ask = {}", add.join(", "));
    }
    if let Some(ref remove) = config.always_ask_remove
        && !remove.is_empty()
    {
        eprintln!("always_ask_remove = {}", remove.join(", "));
    }
    eprintln!("\nConfig file: {}", config_path().display());
    Ok(())
}

// --- Auto-approve subcommands ---

/// Effective always-defer set: built-in defaults + `always_ask`, minus
/// `always_ask_remove`. Mirrors `is_always_deferred` in wisphive_hook.
fn effective_always_ask(config: &UserConfig) -> Vec<String> {
    let removed = config.always_ask_remove.clone().unwrap_or_default();
    let mut set: Vec<String> = wisphive_protocol::DEFAULT_ALWAYS_ASK
        .iter()
        .map(|s| s.to_string())
        .chain(config.always_ask.clone().unwrap_or_default())
        .filter(|t| !removed.contains(t))
        .collect();
    set.dedup();
    set
}

pub fn auto_approve_status() -> Result<()> {
    let config = load();
    let level = config.auto_approve_level.unwrap_or_default();
    eprintln!("Level: {level}");
    if config.auto_approve_dangerous {
        eprintln!(
            "Posture: DANGEROUS — questions/plan-mode are auto-approved too (nothing defers)."
        );
    } else {
        let deferred = effective_always_ask(&config);
        eprintln!("Posture: balanced — these always defer to you regardless of level:");
        for t in &deferred {
            eprintln!("  ? {t}");
        }
    }
    eprintln!("Tools at this level:");
    // Show all tools included by the level
    let all_tools = [
        "Read",
        "Glob",
        "Grep",
        "LS",
        "WebSearch",
        "WebFetch",
        "NotebookRead",
        "Agent",
        "Skill",
        "TaskCreate",
        "TaskUpdate",
        "TaskGet",
        "TaskList",
        "TodoRead",
        "ToolSearch",
        "Edit",
        "Write",
        "NotebookEdit",
        "Bash",
    ];
    for tool in &all_tools {
        if level.includes(tool) {
            eprintln!("  + {tool}");
        }
    }
    if let Some(ref add) = config.auto_approve_add
        && !add.is_empty()
    {
        eprintln!("\nOverrides (added):");
        for t in add {
            eprintln!("  + {t}");
        }
    }
    if let Some(ref remove) = config.auto_approve_remove
        && !remove.is_empty()
    {
        eprintln!("\nOverrides (removed — queued despite level):");
        for t in remove {
            eprintln!("  - {t}");
        }
    }
    Ok(())
}

pub fn auto_approve_level(level_str: &str) -> Result<()> {
    let level: wisphive_protocol::AutoApproveLevel =
        level_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;
    let mut config = load();
    config.auto_approve_level = Some(level);
    save(&config)?;
    eprintln!("auto_approve_level = {level}");
    Ok(())
}

pub fn auto_approve_add(tool: &str) -> Result<()> {
    let mut config = load();
    let add = config.auto_approve_add.get_or_insert_with(Vec::new);
    if !add.iter().any(|t| t == tool) {
        add.push(tool.to_string());
    }
    // Remove from the remove list if present
    if let Some(ref mut remove) = config.auto_approve_remove {
        remove.retain(|t| t != tool);
    }
    save(&config)?;
    eprintln!("Added {tool} to auto-approve overrides");
    Ok(())
}

pub fn auto_approve_remove(tool: &str) -> Result<()> {
    let mut config = load();
    let remove = config.auto_approve_remove.get_or_insert_with(Vec::new);
    if !remove.iter().any(|t| t == tool) {
        remove.push(tool.to_string());
    }
    // Remove from the add list if present
    if let Some(ref mut add) = config.auto_approve_add {
        add.retain(|t| t != tool);
    }
    save(&config)?;
    eprintln!("Removed {tool} from auto-approve (will be queued)");
    Ok(())
}

pub fn auto_approve_reset() -> Result<()> {
    let mut config = load();
    config.auto_approve_level = None;
    config.auto_approve_add = None;
    config.auto_approve_remove = None;
    config.always_ask = None;
    config.always_ask_remove = None;
    config.auto_approve_dangerous = false;
    save(&config)?;
    eprintln!("Auto-approve reset to defaults (level: read, balanced posture)");
    Ok(())
}

/// Apply a named auto-approve posture preset.
///
/// - `balanced` — auto-approve operational tools broadly (level: all) but ALWAYS
///   surface questions/plan-mode/harmful actions for review.
/// - `dangerous` — auto-approve EVERYTHING, including questions (level: all,
///   nothing defers). Use only for fully unattended, trusted runs.
pub fn auto_approve_mode(mode: &str) -> Result<()> {
    let mut config = load();
    match mode {
        "balanced" | "safe" => {
            config.auto_approve_level = Some(wisphive_protocol::AutoApproveLevel::All);
            config.auto_approve_dangerous = false;
            save(&config)?;
            eprintln!(
                "Posture: balanced — level=all for tools, but questions/plan-mode/harmful actions always defer to you."
            );
        }
        "dangerous" | "danger" | "yolo" => {
            config.auto_approve_level = Some(wisphive_protocol::AutoApproveLevel::All);
            config.auto_approve_dangerous = true;
            save(&config)?;
            eprintln!(
                "Posture: DANGEROUS — EVERYTHING is auto-approved, including questions and plan-mode. Nothing will defer to you."
            );
        }
        other => anyhow::bail!("unknown posture: {other} (use 'balanced' or 'dangerous')"),
    }
    eprintln!("Note: restart the daemon for changes to take effect.");
    Ok(())
}

/// Add a tool/event to the always-defer set (e.g. a harmful-action tool).
pub fn auto_approve_defer_add(tool: &str) -> Result<()> {
    let mut config = load();
    // Clear any prior removal so the tool actually defers again.
    if let Some(ref mut remove) = config.always_ask_remove {
        remove.retain(|t| t != tool);
    }
    if !wisphive_protocol::DEFAULT_ALWAYS_ASK.contains(&tool) {
        let add = config.always_ask.get_or_insert_with(Vec::new);
        if !add.iter().any(|t| t == tool) {
            add.push(tool.to_string());
        }
    }
    save(&config)?;
    eprintln!("{tool} will always defer to you (regardless of auto-approve level).");
    Ok(())
}

/// Drop a tool/event from the always-defer set so it follows the normal
/// auto-approve level again.
pub fn auto_approve_defer_remove(tool: &str) -> Result<()> {
    let mut config = load();
    // Drop from operator additions if present.
    if let Some(ref mut add) = config.always_ask {
        add.retain(|t| t != tool);
    }
    // If it's a built-in default, record an explicit removal.
    if wisphive_protocol::DEFAULT_ALWAYS_ASK.contains(&tool) {
        let remove = config.always_ask_remove.get_or_insert_with(Vec::new);
        if !remove.iter().any(|t| t == tool) {
            remove.push(tool.to_string());
        }
    }
    save(&config)?;
    eprintln!("{tool} no longer always-defers; it follows the auto-approve level now.");
    Ok(())
}
