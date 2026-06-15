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
    /// Maximum age in days for daemon log files in `log_dir` (older are pruned at startup).
    pub log_retention_days: u64,
    /// Upper bound (bytes) on the database size for which retention may run a
    /// full `VACUUM`. Above this, VACUUM is skipped (it rewrites the whole DB
    /// and can hang/OOM on a multi-GB file); a WAL checkpoint still runs.
    pub retention_vacuum_max_bytes: u64,
    /// Size (bytes) of the on-disk audit archive above which a non-destructive
    /// alert is raised. Wisphive never auto-deletes audit data (itr#340); it
    /// warns instead. `0` disables the archive-size alert.
    pub archive_alert_max_bytes: u64,
    /// Free-space floor (bytes) on the state filesystem; a low-disk alert is
    /// raised when available space drops below this. `0` disables it.
    pub disk_alert_free_bytes: u64,
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
    /// Extra tools/events that always defer to the agent's native prompt
    /// (questions, plan-mode, elicitations, and operator-added harmful actions),
    /// on top of the built-in default set. See `DEFAULT_ALWAYS_ASK` in
    /// wisphive_hook and itr#380.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_ask: Option<Vec<String>>,
    /// Tools to drop from the built-in always-defer set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_ask_remove: Option<Vec<String>>,
    /// "Dangerous" posture: when true, the always-defer set is ignored and
    /// everything (including questions/plan-mode) is auto-approved per the
    /// level. Off by default; pairs with `auto_approve_level: all`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_approve_dangerous: bool,
    /// Content-aware rules per tool (deny/allow patterns on tool input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_rules: Option<std::collections::HashMap<String, wisphive_protocol::ToolRule>>,
    /// Max rows to keep in decision_log SQLite table (default: 50000).
    #[serde(default)]
    pub retention_max_rows: Option<u64>,
    /// Max age in days for decision_log entries (default: 30).
    #[serde(default)]
    pub retention_max_age_days: Option<u64>,
    /// Max age in days for daemon log files (default: 14).
    #[serde(default)]
    pub log_retention_days: Option<u64>,
    /// Max DB size in MB for which retention may run a full VACUUM (default: 256).
    #[serde(default)]
    pub retention_vacuum_max_mb: Option<u64>,
    /// Audit-archive size in MB above which a non-destructive alert is raised
    /// (default: 10240 = 10 GiB). `0` disables.
    #[serde(default)]
    pub archive_alert_max_mb: Option<u64>,
    /// Low-disk alert threshold in MB of free space (default: 10240 = 10 GiB).
    /// `0` disables.
    #[serde(default)]
    pub disk_alert_free_mb: Option<u64>,
}

fn default_true() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
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

        let hook_timeout_secs = clamp_config(
            "hook_timeout_secs",
            user.hook_timeout_secs.unwrap_or(3600),
            10,
            86_400,
        );
        let agent_timeout_secs = clamp_config(
            "agent_timeout_secs",
            user.agent_timeout_secs.unwrap_or(300),
            10,
            86_400,
        );
        let retention_max_rows = clamp_config(
            "retention_max_rows",
            user.retention_max_rows.unwrap_or(50_000),
            100,
            10_000_000,
        );
        let retention_max_age_days = clamp_config(
            "retention_max_age_days",
            user.retention_max_age_days.unwrap_or(30),
            1,
            3650,
        );
        let log_retention_days = clamp_config(
            "log_retention_days",
            user.log_retention_days.unwrap_or(14),
            1,
            3650,
        );
        let retention_vacuum_max_mb = clamp_config(
            "retention_vacuum_max_mb",
            user.retention_vacuum_max_mb.unwrap_or(256),
            16,
            100_000,
        );
        let retention_vacuum_max_bytes = retention_vacuum_max_mb.saturating_mul(1024 * 1024);
        // Alert thresholds: allow 0 (disabled) up to a generous ceiling. Unlike
        // the retention knobs these never delete anything, so the lower bound is
        // 0 rather than a positive minimum.
        let archive_alert_max_mb = clamp_config(
            "archive_alert_max_mb",
            user.archive_alert_max_mb.unwrap_or(10_240),
            0,
            100_000_000,
        );
        let disk_alert_free_mb = clamp_config(
            "disk_alert_free_mb",
            user.disk_alert_free_mb.unwrap_or(10_240),
            0,
            100_000_000,
        );
        let archive_alert_max_bytes = archive_alert_max_mb.saturating_mul(1024 * 1024);
        let disk_alert_free_bytes = disk_alert_free_mb.saturating_mul(1024 * 1024);

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
            log_retention_days,
            retention_vacuum_max_bytes,
            archive_alert_max_bytes,
            disk_alert_free_bytes,
            home_dir,
        }
    }

    /// Create config using the default ~/.wisphive location.
    pub fn default_location() -> Self {
        let home = dirs_home().join(".wisphive");
        Self::new(home)
    }

    /// Ensure the home directory and log directory exist, and lock the home
    /// directory down to owner-only (0700).
    ///
    /// `create_dir_all` honours the process umask, so a permissive umask could
    /// leave `~/.wisphive` group/world readable; the explicit chmod forces
    /// 0700 so the socket, SQLite DB, audit archive, and config inside are not
    /// reachable by other local users. Defence-in-depth alongside the 0600
    /// socket perms and the peer-credential check in `server.rs` (itr#81).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&self.home_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        std::fs::set_permissions(&self.home_dir, std::fs::Permissions::from_mode(0o700))?;
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
