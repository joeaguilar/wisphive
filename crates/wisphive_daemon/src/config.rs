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
#[derive(serde::Deserialize, serde::Serialize)]
pub struct UserConfig {
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_timeout_secs: Option<u64>,
    /// Auto-approve permission level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_max_rows: Option<u64>,
    /// Max age in days for decision_log entries (default: 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_max_age_days: Option<u64>,
    /// Max age in days for daemon log files (default: 14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_retention_days: Option<u64>,
    /// Max DB size in MB for which retention may run a full VACUUM (default: 256).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_vacuum_max_mb: Option<u64>,
    /// Audit-archive size in MB above which a non-destructive alert is raised
    /// (default: 10240 = 10 GiB). `0` disables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_alert_max_mb: Option<u64>,
    /// Low-disk alert threshold in MB of free space (default: 10240 = 10 GiB).
    /// `0` disables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_alert_free_mb: Option<u64>,
    /// Keys this struct doesn't model (event toggles like `auto_approve_stop`,
    /// future additions). Captured so a load→save round-trip never strips them
    /// — the hook reads several of these as raw top-level keys (itr#361).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for UserConfig {
    /// Must match the serde field defaults — the derived impl gave
    /// `notifications: false` while a missing key deserializes to `true`, so
    /// every default-fallback path silently flipped notifications off.
    fn default() -> Self {
        serde_json::from_str("{}").expect("empty object deserializes to defaults")
    }
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
            Ok(content) => match serde_json::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    // Corrupt (present but unparseable) config is an operator
                    // error, not a reason to silently run permissive defaults
                    // (itr#308). Warn loudly on both channels — tracing may not
                    // be initialized yet at config-load time — and never write
                    // back over the user's file.
                    let msg = format!(
                        "CORRUPT CONFIG: {} is not valid JSON ({e}); \
                         running with built-in defaults. The file was NOT modified — \
                         fix or remove it, then restart the daemon.",
                        path.display()
                    );
                    eprintln!("wisphive: {msg}");
                    tracing::error!("{msg}");
                    UserConfig::default()
                }
            },
            Err(_) => UserConfig::default(),
        }
    }
}

/// Atomically replace `path` with `body`: write to a same-directory temp file
/// (0600), fsync, then rename over the target. A crash mid-write can never
/// leave a truncated config behind (itr#92).
pub fn write_config_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    // pid + per-process counter keeps concurrent writers (parallel hook
    // connections) off each other's temp files.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = parent.join(format!(".{file_name}.{}.{seq}.tmp", std::process::id()));

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// Error from [`update_config_json`].
#[derive(Debug)]
pub enum ConfigUpdateError {
    /// The file exists but is not valid JSON. The update is refused so a
    /// read-modify-write can never clobber a corrupt-but-recoverable config
    /// (itr#308).
    Corrupt(serde_json::Error),
    /// The file's top level is valid JSON but not an object.
    NotAnObject,
    /// The mutation refused the update (e.g. an existing key has the wrong
    /// type). Nothing is written — no false "saved" (itr#308 posture).
    Rejected(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ConfigUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt(e) => write!(
                f,
                "config file is not valid JSON ({e}); refusing to overwrite it — fix or remove the file"
            ),
            Self::NotAnObject => write!(f, "config file top level is not a JSON object"),
            Self::Rejected(reason) => write!(f, "config update refused: {reason}"),
            Self::Io(e) => write!(f, "config I/O error: {e}"),
        }
    }
}

impl std::error::Error for ConfigUpdateError {}

/// Read-modify-write a JSON config file, preserving every key the mutation
/// doesn't touch. This is the single owner of "edit one key in config.json"
/// (itr#358/#360/#361): the file is read as raw JSON (unknown keys survive),
/// `mutate` edits the top-level object in place, and the result is written
/// atomically. A missing file starts from `{}`; a corrupt file refuses the
/// update instead of being overwritten. `mutate` returning `Err` aborts the
/// update before anything is written (no false "saved").
pub fn update_config_json(
    path: &Path,
    mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String>,
) -> Result<(), ConfigUpdateError> {
    let mut root: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(ConfigUpdateError::Corrupt)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(ConfigUpdateError::Io(e)),
    };
    let obj = root.as_object_mut().ok_or(ConfigUpdateError::NotAnObject)?;
    mutate(obj).map_err(ConfigUpdateError::Rejected)?;
    let body = serde_json::to_string_pretty(&root).expect("JSON value always serializes");
    write_config_atomic(path, &body).map_err(ConfigUpdateError::Io)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_round_trip_preserves_unknown_keys() {
        // itr#361: the hook reads event toggles (auto_approve_user_prompt etc.)
        // as raw top-level keys UserConfig doesn't model. A load→save through
        // UserConfig must not strip them — losing auto_approve_user_prompt:false
        // silently weakens the operator's gating policy.
        let raw = serde_json::json!({
            "notifications": true,
            "auto_approve_level": "all",
            "auto_approve_user_prompt": false,
            "auto_approve_stop": true,
            "some_future_key": {"nested": [1, 2, 3]},
        });
        let config: UserConfig = serde_json::from_value(raw).unwrap();
        let round_tripped = serde_json::to_value(&config).unwrap();

        assert_eq!(round_tripped["auto_approve_user_prompt"], false);
        assert_eq!(round_tripped["auto_approve_stop"], true);
        assert_eq!(round_tripped["some_future_key"]["nested"][1], 2);
        assert_eq!(round_tripped["auto_approve_level"], "all");
    }

    #[test]
    fn update_config_json_preserves_untouched_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "tool_rules": {"Bash": {"deny_patterns": ["rm -rf"], "allow_patterns": []}},
                "auto_approve_user_prompt": false,
            })
            .to_string(),
        )
        .unwrap();

        update_config_json(&path, |obj| {
            obj.insert("auto_approve_level".into(), "read".into());
            Ok(())
        })
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["auto_approve_level"], "read");
        assert_eq!(after["tool_rules"]["Bash"]["deny_patterns"][0], "rm -rf");
        assert_eq!(after["auto_approve_user_prompt"], false);
    }

    #[test]
    fn update_config_json_starts_from_empty_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        update_config_json(&path, |obj| {
            obj.insert("auto_approve_add".into(), serde_json::json!(["Bash"]));
            Ok(())
        })
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["auto_approve_add"][0], "Bash");
    }

    #[test]
    fn update_config_json_refuses_corrupt_file_and_leaves_it_untouched() {
        // itr#308: a corrupt config must never be silently replaced — the
        // operator's (recoverable) content would be destroyed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not json !!").unwrap();

        let err = update_config_json(&path, |obj| {
            obj.insert("auto_approve_level".into(), "all".into());
            Ok(())
        })
        .expect_err("corrupt config must refuse the update");
        assert!(matches!(err, ConfigUpdateError::Corrupt(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json !!");
    }

    #[test]
    fn corrupt_user_config_loads_defaults_without_writing_back() {
        // itr#308: corrupt config.json → loud fallback to defaults, and the
        // file on disk is never modified.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "definitely not json").unwrap();

        let config = DaemonConfig::new(dir.path().to_path_buf());
        assert_eq!(config.hook_timeout_secs, 3600);
        assert!(config.notifications_enabled);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "definitely not json"
        );
    }

    #[test]
    fn write_config_atomic_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write_config_atomic(&path, "{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
