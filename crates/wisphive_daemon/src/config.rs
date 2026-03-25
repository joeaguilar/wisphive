use std::path::{Path, PathBuf};

/// Daemon configuration — paths and tuning parameters.
pub struct DaemonConfig {
    /// Root directory for all Wisphive state: ~/.wisphive
    pub home_dir: PathBuf,
    /// Unix socket path: ~/.wisphive/wisphive.sock
    pub socket_path: PathBuf,
    /// PID file path: ~/.wisphive/wisphive.pid
    pub pid_path: PathBuf,
    /// SQLite database path: ~/.wisphive/wisphive.db
    pub db_path: PathBuf,
    /// Mode file path: ~/.wisphive/mode
    pub mode_path: PathBuf,
    /// Log directory: ~/.wisphive/logs/
    pub log_dir: PathBuf,
    /// Maximum time a hook can block waiting for a decision (seconds).
    pub hook_timeout_secs: u64,
    /// Whether to send desktop notifications for pending decisions.
    pub notifications_enabled: bool,
    /// Seconds of inactivity before an agent is reaped from the registry.
    pub agent_timeout_secs: u64,
    /// Maximum rows to keep in decision_log (oldest archived to JSONL, then deleted).
    pub retention_max_rows: u64,
    /// Maximum age in days for decision_log entries (older archived and deleted).
    pub retention_max_age_days: u64,
}

/// User-editable config loaded from ~/.wisphive/config.json.
#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct UserConfig {
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default)]
    pub hook_timeout_secs: Option<u64>,
    #[serde(default)]
    pub agent_timeout_secs: Option<u64>,
    /// Auto-approve permission level.
    #[serde(default)]
    pub auto_approve_level: Option<wisphive_protocol::AutoApproveLevel>,
    /// Extra tools to auto-approve on top of the level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve_add: Option<Vec<String>>,
    /// Tools to exclude from auto-approve despite the level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve_remove: Option<Vec<String>>,
    /// Content-aware rules per tool (deny/allow patterns on tool input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_rules: Option<std::collections::HashMap<String, wisphive_protocol::ToolRule>>,
    /// Max rows to keep in decision_log SQLite table (default: 50000).
    #[serde(default)]
    pub retention_max_rows: Option<u64>,
    /// Max age in days for decision_log entries (default: 30).
    #[serde(default)]
    pub retention_max_age_days: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// Clamp a config value to a valid range, logging a warning if clamped.
fn clamp_config(name: &str, value: u64, min: u64, max: u64) -> u64 {
    if value < min {
        tracing::warn!(name, value, min, "config value below minimum, clamping");
        min
    } else if value > max {
        tracing::warn!(name, value, max, "config value above maximum, clamping");
        max
    } else {
        value
    }
}

impl DaemonConfig {
    /// Create config rooted at the given home directory.
    ///
    /// Loads user overrides from `<home_dir>/config.json` if present.
    pub fn new(home_dir: PathBuf) -> Self {
        let user = Self::load_user_config(&home_dir);

        let hook_timeout_secs = clamp_config("hook_timeout_secs", user.hook_timeout_secs.unwrap_or(3600), 10, 86_400);
        let agent_timeout_secs = clamp_config("agent_timeout_secs", user.agent_timeout_secs.unwrap_or(300), 10, 86_400);
        let retention_max_rows = clamp_config("retention_max_rows", user.retention_max_rows.unwrap_or(50_000), 100, 10_000_000);
        let retention_max_age_days = clamp_config("retention_max_age_days", user.retention_max_age_days.unwrap_or(30), 1, 3650);

        Self {
            socket_path: home_dir.join("wisphive.sock"),
            pid_path: home_dir.join("wisphive.pid"),
            db_path: home_dir.join("wisphive.db"),
            mode_path: home_dir.join("mode"),
            log_dir: home_dir.join("logs"),
            hook_timeout_secs,
            notifications_enabled: user.notifications,
            agent_timeout_secs,
            retention_max_rows,
            retention_max_age_days,
            home_dir,
        }
    }

    /// Create config using the default ~/.wisphive location.
    pub fn default_location() -> Self {
        let home = dirs_home().join(".wisphive");
        Self::new(home)
    }

    /// Ensure the home directory and log directory exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.home_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        Ok(())
    }

    /// Path to the user config file.
    pub fn config_json_path(&self) -> PathBuf {
        self.home_dir.join("config.json")
    }

    fn load_user_config(home_dir: &Path) -> UserConfig {
        let path = home_dir.join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => UserConfig::default(),
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Expand a tilde prefix to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs_home().join(rest)
    } else {
        Path::new(path).to_path_buf()
    }
}
