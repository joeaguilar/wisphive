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
        _ => eprintln!("unknown config key: {key}. Valid keys: notifications, hook_timeout_secs"),
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
        _ => anyhow::bail!("unknown config key: {key}. Valid keys: notifications, hook_timeout_secs"),
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
    eprintln!("\nConfig file: {}", config_path().display());
    Ok(())
}
