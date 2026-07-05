use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
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
        // SQLite defaults `foreign_keys=OFF` per-connection. Enabling it on
        // the connect options applies to every pooled connection so the
        // `ON DELETE CASCADE` on web_passkeys → web_devices is actually
        // enforced at runtime.
        let url = format!("sqlite:{}?mode=rwc", path);
        let opts = SqliteConnectOptions::from_str(&url)?.foreign_keys(true);
        let pool = SqlitePool::connect_with(opts).await?;

        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
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
