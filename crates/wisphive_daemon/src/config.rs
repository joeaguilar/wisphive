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
}

impl DaemonConfig {
    /// Create config rooted at the given home directory.
    pub fn new(home_dir: PathBuf) -> Self {
        Self {
            socket_path: home_dir.join("wisphive.sock"),
            pid_path: home_dir.join("wisphive.pid"),
            db_path: home_dir.join("wisphive.db"),
            mode_path: home_dir.join("mode"),
            log_dir: home_dir.join("logs"),
            hook_timeout_secs: 3600,
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
