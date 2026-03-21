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
        _ => eprintln!("unknown config key: {key}. Valid: notifications, hook_timeout_secs, agent_timeout_secs, auto_approve_level"),
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
            let secs: u64 = value.parse().context("hook_timeout_secs must be a number")?;
            config.hook_timeout_secs = Some(secs);
        }
        "agent_timeout_secs" => {
            let secs: u64 = value.parse().context("agent_timeout_secs must be a number")?;
            config.agent_timeout_secs = Some(secs);
        }
        "auto_approve_level" => {
            let level: wisphive_protocol::AutoApproveLevel = value.parse()
                .map_err(|e: String| anyhow::anyhow!(e))?;
            config.auto_approve_level = Some(level);
        }
        _ => anyhow::bail!("unknown config key: {key}. Valid: notifications, hook_timeout_secs, agent_timeout_secs, auto_approve_level"),
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
    if let Some(ref add) = config.auto_approve_add {
        if !add.is_empty() {
            eprintln!("auto_approve_add = {}", add.join(", "));
        }
    }
    if let Some(ref remove) = config.auto_approve_remove {
        if !remove.is_empty() {
            eprintln!("auto_approve_remove = {}", remove.join(", "));
        }
    }
    eprintln!("\nConfig file: {}", config_path().display());
    Ok(())
}

// --- Auto-approve subcommands ---

pub fn auto_approve_status() -> Result<()> {
    let config = load();
    let level = config.auto_approve_level.unwrap_or_default();
    eprintln!("Level: {level}");
    eprintln!("Tools at this level:");
    // Show all tools included by the level
    let all_tools = ["Read", "Glob", "Grep", "LS", "WebSearch", "WebFetch",
        "NotebookRead", "Agent", "Skill", "TaskCreate", "TaskUpdate",
        "TaskGet", "TaskList", "TodoRead", "ToolSearch",
        "Edit", "Write", "NotebookEdit", "Bash"];
    for tool in &all_tools {
        if level.includes(tool) {
            eprintln!("  + {tool}");
        }
    }
    if let Some(ref add) = config.auto_approve_add {
        if !add.is_empty() {
            eprintln!("\nOverrides (added):");
            for t in add {
                eprintln!("  + {t}");
            }
        }
    }
    if let Some(ref remove) = config.auto_approve_remove {
        if !remove.is_empty() {
            eprintln!("\nOverrides (removed — queued despite level):");
            for t in remove {
                eprintln!("  - {t}");
            }
        }
    }
    Ok(())
}

pub fn auto_approve_level(level_str: &str) -> Result<()> {
    let level: wisphive_protocol::AutoApproveLevel = level_str.parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
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
    save(&config)?;
    eprintln!("Auto-approve reset to defaults (level: read)");
    Ok(())
}
