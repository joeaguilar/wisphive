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
}

/// User-editable config loaded from ~/.wisphive/config.json.
#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct UserConfig {
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_timeout")]
    pub hook_timeout_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> Option<u64> {
    None
}

impl DaemonConfig {
    /// Create config rooted at the given home directory.
    ///
    /// Loads user overrides from `<home_dir>/config.json` if present.
    pub fn new(home_dir: PathBuf) -> Self {
        let user = Self::load_user_config(&home_dir);
        Self {
            socket_path: home_dir.join("wisphive.sock"),
            pid_path: home_dir.join("wisphive.pid"),
            db_path: home_dir.join("wisphive.db"),
            mode_path: home_dir.join("mode"),
            log_dir: home_dir.join("logs"),
            hook_timeout_secs: user.hook_timeout_secs.unwrap_or(3600),
            notifications_enabled: user.notifications,
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
