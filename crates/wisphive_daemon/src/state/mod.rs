use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::info;

mod decisions;
mod migrate;
mod retention;
mod summaries;
mod terminals;
mod web_auth;
mod web_passkeys;

pub use decisions::{AttachedResult, AutoApprovedEntry};
pub use retention::RetentionOutcome;
pub use web_auth::{WebAuditRow, WebAuthError, WebAuthResult, WebDeviceRow};
pub use web_passkeys::WebPasskeyRow;

/// Size cap for the `decision_log.jsonl` archive sink before it is rotated.
const ARCHIVE_SINK_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Row shape returned by decision_log queries (13 columns).
type DecisionLogRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Row shape for pending_decisions lookups (8 columns).
#[allow(dead_code)]
type PendingRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

/// If `path` is larger than `max_bytes`, rename it to a timestamped sibling so
/// a fresh file is started. The rotated segment keeps the same directory (the
/// daemon log dir). The decision archive and its rotated segments are NOT
/// reaped by `logging::prune_old_files` (audit data); a durable home + dedicated
/// retention for them is tracked in #340.
///
/// Non-fatal: a failure can never abort retention, but it is surfaced via
/// `warn!` rather than silently swallowed — a persistent rename failure means
/// the sink grows unbounded past its cap, which must not be invisible (#341).
fn rotate_if_large(path: &std::path::Path, max_bytes: u64) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return, // nothing to rotate
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "archive rotation: stat failed");
            return;
        }
    };
    if meta.len() <= max_bytes {
        return;
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f");
    let rotated = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.{stamp}")),
        None => path.with_extension(format!("{stamp}")),
    };
    if let Err(e) = std::fs::rename(path, &rotated) {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "archive rotation: rename failed; sink will exceed its size cap until this clears"
        );
    }
}

/// Manages the SQLite state database for crash recovery and audit.
///
/// Cheap to clone — the internal [`SqlitePool`] is an Arc-backed connection
/// pool, so cloning hands out another handle to the same pool rather than
/// opening new connections. Web auth plumbing relies on this to share a
/// single DB handle between the axum middleware and per-request handlers.
#[derive(Clone)]
pub struct StateDb {
    pool: SqlitePool,
}

impl StateDb {
    /// Open (or create) the database at the given path. Call this from the
    /// daemon process only — it runs startup hooks that are daemon-specific.
    /// CLI callers sharing the DB must use [`Self::open_client`] instead.
    pub async fn open(path: &str) -> Result<Self> {
        let db = Self::open_raw(path).await?;
        // Any terminal session still marked running at daemon startup
        // belongs to a prior daemon instance whose PTY is gone. Mark
        // orphaned so replay still works but clients know the live stream
        // is unreachable.
        //
        // CRITICAL — this MUST NOT run from non-daemon processes: a live
        // daemon's running PTYs would all get flipped to orphaned and the
        // daemon would continue writing events to rows the DB now considers
        // ended. That's why the CLI uses `open_client` below.
        db.mark_running_terminals_orphaned().await?;
        info!("state database ready at {}", path);
        Ok(db)
    }

    /// Open (or create) the database for a read/write client that is NOT
    /// the daemon (CLI admin commands, migrations, tooling).
    ///
    /// Runs the same schema migration as [`Self::open`] — idempotent
    /// `CREATE TABLE IF NOT EXISTS` + tolerant `ALTER TABLE ... ok()` —
    /// but deliberately skips [`Self::mark_running_terminals_orphaned`],
    /// which would otherwise corrupt the state of a running daemon's PTY
    /// sessions (itr#215 review, sec#5: "CLI will corrupt daemon state").
    pub async fn open_client(path: &str) -> Result<Self> {
        let db = Self::open_raw(path).await?;
        info!("state database ready (client mode) at {}", path);
        Ok(db)
    }

    /// Shared open + migrate path used by both [`Self::open`] and
    /// [`Self::open_client`]. Not public — callers must choose between
    /// "daemon startup hooks" and "no startup hooks" explicitly.
    async fn open_raw(path: &str) -> Result<Self> {
        let filesystem_path = database_filesystem_path(path);
        let secure_location = match filesystem_path {
            Some(path) => Some(
                SecureDatabaseLocation::preflight(path.clone())
                    .await
                    .with_context(|| format!("secure SQLite preflight for {}", path.display()))?,
            ),
            None => None,
        };

        // SQLite defaults `foreign_keys=OFF` per-connection. Enabling it on
        // the connect options applies to every pooled connection so the
        // `ON DELETE CASCADE` on web_passkeys → web_devices is actually
        // enforced at runtime. Select WAL before migrations so the first
        // schema writes create live sidecars while the pool is open.
        let url = format!("sqlite:{}?mode=rwc", path);
        let opts = SqliteConnectOptions::from_str(&url)?
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePool::connect_with(opts).await?;

        let db = Self { pool };
        db.migrate().await?;
        if let Some(location) = secure_location {
            let path = location.database_path().to_owned();
            location
                .postflight()
                .await
                .with_context(|| format!("secure SQLite postflight for {}", path.display()))?;
        }
        Ok(db)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn database_filesystem_path(path: &str) -> Option<PathBuf> {
    if path == ":memory:" || path.starts_with("file::memory:") {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(unix)]
struct SecureDatabaseLocation {
    directory: std::fs::File,
    parent_path: PathBuf,
    database_path: PathBuf,
    database_name: std::ffi::OsString,
    parent_device: u64,
    parent_inode: u64,
}

#[cfg(unix)]
impl SecureDatabaseLocation {
    async fn preflight(database_path: PathBuf) -> std::io::Result<Self> {
        tokio::task::spawn_blocking(move || Self::preflight_sync(database_path))
            .await
            .map_err(|error| std::io::Error::other(format!("database preflight task: {error}")))?
    }

    fn preflight_sync(database_path: PathBuf) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;

        let parent_path = database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        let database_name = database_path
            .file_name()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "database path has no file name",
                )
            })?
            .to_owned();
        let directory = open_secure_database_parent(&parent_path)?;
        let metadata = directory.metadata()?;
        let location = Self {
            directory,
            parent_path,
            database_path,
            database_name,
            parent_device: metadata.dev(),
            parent_inode: metadata.ino(),
        };

        location.secure_entry("", true)?;
        location.secure_entry("-wal", false)?;
        location.secure_entry("-shm", false)?;
        Ok(location)
    }

    fn database_path(&self) -> &Path {
        &self.database_path
    }

    async fn postflight(self) -> std::io::Result<()> {
        tokio::task::spawn_blocking(move || self.postflight_sync())
            .await
            .map_err(|error| std::io::Error::other(format!("database postflight task: {error}")))?
    }

    fn postflight_sync(self) -> std::io::Result<()> {
        use std::os::unix::fs::MetadataExt;

        // SQLite reopens by pathname. Re-open the final parent without
        // following symlinks and require the same directory identity held
        // since preflight before accepting SQLite's newly-created sidecars.
        let reopened_parent = open_secure_database_parent(&self.parent_path)?;
        let reopened_metadata = reopened_parent.metadata()?;
        if reopened_metadata.dev() != self.parent_device
            || reopened_metadata.ino() != self.parent_inode
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "database parent changed during SQLite open",
            ));
        }

        self.secure_entry("", false)?;
        self.secure_entry("-wal", false)?;
        self.secure_entry("-shm", false)?;
        Ok(())
    }

    fn secure_entry(&self, suffix: &str, create_main: bool) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt;

        let mut name = self.database_name.clone();
        name.push(suffix);
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "database file name contains a NUL byte",
            )
        })?;
        if let Some(file) = open_database_entry_at(&self.directory, &name, create_main)? {
            validate_and_repair_database_file(&file, &name)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn open_secure_database_parent(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "database parent is not a directory",
        ));
    }

    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "database parent is owned by uid {}, expected effective uid {effective_uid}",
                metadata.uid()
            ),
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "database parent permissions are {:#06o}; group/world write is forbidden",
                metadata.mode() & 0o7777
            ),
        ));
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_database_entry_at(
    directory: &std::fs::File,
    name: &std::ffi::CStr,
    create_if_missing: bool,
) -> std::io::Result<Option<std::fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let existing_flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    // SAFETY: the directory descriptor and NUL-terminated relative name stay
    // valid for the call. A successful descriptor is transferred exactly once.
    let mut fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), existing_flags) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
        if !create_if_missing {
            return Ok(None);
        }

        // O_EXCL makes creation fail instead of following a same-UID path swap.
        // Cross-UID swaps are already excluded by the secure parent directory.
        // SAFETY: arguments remain valid; a successful fd is owned below.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                existing_flags | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    // SAFETY: `openat` returned a fresh owned descriptor above.
    Ok(Some(unsafe { std::fs::File::from_raw_fd(fd) }))
}

#[cfg(unix)]
fn validate_and_repair_database_file(
    file: &std::fs::File,
    name: &std::ffi::CStr,
) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("SQLite path {:?} is not a regular file", name),
        ));
    }
    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SQLite path {:?} is owned by uid {}, expected effective uid {effective_uid}",
                name,
                metadata.uid()
            ),
        ));
    }

    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let repaired = file.metadata()?;
    if repaired.mode() & 0o7777 != 0o600 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SQLite path {:?} permissions are {:#06o}, expected 0o0600",
                name,
                repaired.mode() & 0o7777
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
struct SecureDatabaseLocation {
    database_path: PathBuf,
}

#[cfg(not(unix))]
impl SecureDatabaseLocation {
    async fn preflight(database_path: PathBuf) -> std::io::Result<Self> {
        Ok(Self { database_path })
    }

    fn database_path(&self) -> &Path {
        &self.database_path
    }

    async fn postflight(self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Shared test helpers used by multiple per-domain test modules.
#[cfg(test)]
pub(crate) mod test_support {
    use super::StateDb;
    use wisphive_protocol::{AgentType, DecisionRequest, HookEventType};

    /// Create an in-memory StateDb for testing.
    pub(crate) async fn test_db() -> StateDb {
        StateDb::open(":memory:").await.unwrap()
    }

    pub(crate) fn make_request(tool: &str, agent_id: &str, project: &str) -> DecisionRequest {
        DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: agent_id.into(),
            agent_type: AgentType::ClaudeCode,
            project: std::path::PathBuf::from(project),
            tool_name: tool.into(),
            tool_input: serde_json::json!({"command": "test"}),
            timestamp: chrono::Utc::now(),
            hook_event_name: HookEventType::PreToolUse,
            tool_use_id: None,
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
        }
    }

    pub(crate) fn make_request_with_tool_use_id(
        tool: &str,
        agent_id: &str,
        tool_use_id: &str,
    ) -> DecisionRequest {
        DecisionRequest {
            id: uuid::Uuid::new_v4(),
            agent_id: agent_id.into(),
            agent_type: AgentType::ClaudeCode,
            project: std::path::PathBuf::from("/test"),
            tool_name: tool.into(),
            tool_input: serde_json::json!({"command": "test"}),
            timestamp: chrono::Utc::now(),
            hook_event_name: HookEventType::PreToolUse,
            tool_use_id: Some(tool_use_id.into()),
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
        }
    }
}

#[cfg(all(test, unix))]
mod permissions_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    async fn open_database(database_path: &Path, client_open: bool) -> Result<StateDb> {
        let path = database_path.to_string_lossy();
        if client_open {
            StateDb::open_client(&path)
                .await
                .context("open client database")
        } else {
            StateDb::open(&path).await.context("open daemon database")
        }
    }

    async fn assert_live_files_owner_only(db: &StateDb, database_path: &Path) {
        // Hold a pool connection while inspecting the live WAL companions;
        // SQLite may remove them after the final connection closes.
        let _connection = db.pool().acquire().await.expect("hold database open");
        for suffix in ["", "-wal", "-shm"] {
            let file_path = if suffix.is_empty() {
                database_path.to_owned()
            } else {
                sqlite_sidecar_path(database_path, suffix)
            };
            let metadata = tokio::fs::metadata(&file_path)
                .await
                .unwrap_or_else(|error| panic!("{} missing: {error}", file_path.display()));
            assert_eq!(
                metadata.permissions().mode() & 0o7777,
                0o600,
                "{} must be owner-only",
                file_path.display()
            );
        }
    }

    fn create_file_with_mode(path: &Path, mode: u32) {
        std::fs::write(path, b"").expect("create test database artifact");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("set test artifact permissions");
    }

    async fn assert_fresh_database_files_owner_only(client_open: bool) {
        let dir = tempfile::tempdir().expect("create database tempdir");
        let database_path = dir.path().join("wisphive.db");
        let db = open_database(&database_path, client_open)
            .await
            .expect("open fresh database");
        assert_live_files_owner_only(&db, &database_path).await;
    }

    #[tokio::test]
    async fn daemon_open_creates_main_wal_and_shm_as_owner_only() {
        assert_fresh_database_files_owner_only(false).await;
    }

    #[tokio::test]
    async fn client_open_creates_main_wal_and_shm_as_owner_only() {
        assert_fresh_database_files_owner_only(true).await;
    }

    #[tokio::test]
    async fn daemon_and_client_open_repair_loose_main_wal_and_shm_permissions() {
        for client_open in [false, true] {
            let dir = tempfile::tempdir().expect("create database tempdir");
            let database_path = dir.path().join("wisphive.db");
            create_file_with_mode(&database_path, 0o6777);
            create_file_with_mode(&sqlite_sidecar_path(&database_path, "-wal"), 0o6777);
            create_file_with_mode(&sqlite_sidecar_path(&database_path, "-shm"), 0o6777);

            let db = open_database(&database_path, client_open)
                .await
                .expect("repair loose database artifacts");
            assert_live_files_owner_only(&db, &database_path).await;
        }
    }

    #[tokio::test]
    async fn symlink_database_artifacts_are_rejected_without_touching_targets() {
        for suffix in ["", "-wal", "-shm"] {
            let dir = tempfile::tempdir().expect("create database tempdir");
            let database_path = dir.path().join("wisphive.db");
            if !suffix.is_empty() {
                create_file_with_mode(&database_path, 0o600);
            }

            let target = dir.path().join("target");
            std::fs::write(&target, b"test").expect("create symlink target");
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
                .expect("set symlink target permissions");
            let artifact = if suffix.is_empty() {
                database_path.clone()
            } else {
                sqlite_sidecar_path(&database_path, suffix)
            };
            symlink(&target, &artifact).expect("create artifact symlink");

            assert!(open_database(&database_path, true).await.is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"test");
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
                0o644,
                "symlink target for {suffix:?} must be untouched"
            );
        }
    }

    #[tokio::test]
    async fn nonregular_database_artifacts_are_rejected() {
        for suffix in ["", "-wal", "-shm"] {
            let dir = tempfile::tempdir().expect("create database tempdir");
            let database_path = dir.path().join("wisphive.db");
            if !suffix.is_empty() {
                create_file_with_mode(&database_path, 0o600);
            }
            let artifact = if suffix.is_empty() {
                database_path.clone()
            } else {
                sqlite_sidecar_path(&database_path, suffix)
            };
            std::fs::create_dir(&artifact).expect("create nonregular artifact");

            assert!(open_database(&database_path, true).await.is_err());
            assert!(artifact.is_dir(), "nonregular {suffix:?} must be untouched");
        }
    }

    #[tokio::test]
    async fn symlink_and_group_writable_parents_are_rejected_before_creation() {
        let root = tempfile::tempdir().expect("create database tempdir");
        let real_parent = root.path().join("real");
        std::fs::create_dir(&real_parent).expect("create real parent");
        let symlink_parent = root.path().join("linked");
        symlink(&real_parent, &symlink_parent).expect("create parent symlink");
        assert!(
            open_database(&symlink_parent.join("wisphive.db"), true)
                .await
                .is_err()
        );
        assert!(!real_parent.join("wisphive.db").exists());

        std::fs::set_permissions(&real_parent, std::fs::Permissions::from_mode(0o720))
            .expect("make parent group-writable");
        assert!(
            open_database(&real_parent.join("wisphive.db"), true)
                .await
                .is_err()
        );
        assert!(!real_parent.join("wisphive.db").exists());
    }
}
