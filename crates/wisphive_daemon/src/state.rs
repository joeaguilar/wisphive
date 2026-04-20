use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;
use tracing::info;
use wisphive_protocol::{TerminalDirection, TerminalSessionMeta, TerminalStatus};

/// Typed error surface for the web-auth helpers. Auth callers need to
/// distinguish `NotFound` (→ 401/404) from `Duplicate` (→ 409) from `Db`
/// (→ 500) and from `Revoked` (→ 401 + throttle bump). Using `anyhow` here
/// would collapse those into stringly-typed guesses.
#[derive(Debug, thiserror::Error)]
pub enum WebAuthError {
    /// No row matched the lookup (device id, token hash, passkey id, etc.).
    #[error("web auth target not found")]
    NotFound,
    /// The device exists but has been revoked.
    #[error("web device is revoked")]
    Revoked,
    /// Unique-constraint violation (e.g. duplicate device id or token hash).
    #[error("web auth duplicate")]
    Duplicate,
    /// Underlying database / sqlx error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl WebAuthError {
    /// Classify a sqlx error, promoting UNIQUE constraint failures to
    /// `Duplicate` so callers can map them to 409 without string matching.
    fn from_sqlx(err: sqlx::Error) -> Self {
        if let Some(db_err) = err.as_database_error()
            && db_err.message().contains("UNIQUE constraint failed")
        {
            return Self::Duplicate;
        }
        Self::Db(err)
    }
}

pub type WebAuthResult<T> = std::result::Result<T, WebAuthError>;

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

/// Pending row extended with terminal_session_id (9 columns).
type PendingRowWithTerm = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Row shape for session aggregate queries (8 columns).
type SessionRow = (String, String, String, String, String, i64, i64, i64);

/// Row shape for `web_devices` fetches that include `revoked_at`.
type WebDeviceFullRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Row shape for `web_devices` lookups that only load active-device fields.
type WebDeviceActiveRow = (String, String, String, Option<String>, Option<String>);

/// Row shape for `web_passkeys` queries.
type WebPasskeyRowRaw = (
    String,
    String,
    Vec<u8>,
    i64,
    Option<String>,
    String,
    Option<String>,
);

/// Row shape for `web_audit` queries.
type WebAuditRowRaw = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Parameters for logging an auto-approved tool call.
pub struct AutoApprovedEntry<'a> {
    pub agent_id: &'a str,
    pub agent_type: &'a str,
    pub project: &'a str,
    pub tool_name: &'a str,
    pub tool_input: &'a str,
    pub timestamp: &'a str,
    pub tool_use_id: Option<&'a str>,
    pub hook_event_name: Option<&'a str>,
}

/// A row from `web_devices`. `revoked_at` is `None` for active devices.
#[derive(Debug, Clone)]
pub struct WebDeviceRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub last_ip: Option<String>,
    pub revoked_at: Option<String>,
}

/// A row from `web_passkeys`.
#[derive(Debug, Clone)]
pub struct WebPasskeyRow {
    pub id: String,
    pub device_id: String,
    pub public_key: Vec<u8>,
    pub sign_count: i64,
    pub transports: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A row from `web_audit`.
#[derive(Debug, Clone)]
pub struct WebAuditRow {
    pub id: i64,
    pub at: String,
    pub event: String,
    pub device_id: Option<String>,
    pub ip: Option<String>,
    pub detail: Option<String>,
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

    /// Run schema migrations.
    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_decisions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                agent_type TEXT NOT NULL,
                project TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                tool_input TEXT NOT NULL,
                timestamp TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS decision_log (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                agent_type TEXT NOT NULL,
                project TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                tool_input TEXT NOT NULL,
                decision TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                resolved_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Add tool_result column (idempotent — ignore if already exists)
        sqlx::query("ALTER TABLE decision_log ADD COLUMN tool_result TEXT")
            .execute(&self.pool)
            .await
            .ok();

        // Add permission columns (idempotent)
        sqlx::query("ALTER TABLE pending_decisions ADD COLUMN permission_suggestions TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN selected_permission TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN auto_approved INTEGER DEFAULT 0")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN tool_use_id TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE pending_decisions ADD COLUMN tool_use_id TEXT")
            .execute(&self.pool)
            .await
            .ok();

        // Add hook_event_name columns (idempotent)
        sqlx::query(
            "ALTER TABLE pending_decisions ADD COLUMN hook_event_name TEXT DEFAULT 'PreToolUse'",
        )
        .execute(&self.pool)
        .await
        .ok();
        sqlx::query(
            "ALTER TABLE decision_log ADD COLUMN hook_event_name TEXT DEFAULT 'PreToolUse'",
        )
        .execute(&self.pool)
        .await
        .ok();

        // Add terminal_session_id columns for correlating decisions with
        // wisphive-managed terminal sessions (idempotent).
        sqlx::query("ALTER TABLE pending_decisions ADD COLUMN terminal_session_id TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN terminal_session_id TEXT")
            .execute(&self.pool)
            .await
            .ok();

        // Terminal session metadata
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS terminal_sessions (
                id TEXT PRIMARY KEY,
                label TEXT,
                command TEXT NOT NULL,
                args TEXT NOT NULL,
                cwd TEXT NOT NULL,
                env_json TEXT,
                cols INTEGER NOT NULL,
                rows INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                exit_code INTEGER,
                status TEXT NOT NULL DEFAULT 'running'
            )",
        )
        .execute(&self.pool)
        .await?;

        // Sidebar-grouping columns added after the table was introduced.
        // ALTER fails if the column already exists, which is fine — ignore.
        sqlx::query("ALTER TABLE terminal_sessions ADD COLUMN group_name TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query(
            "ALTER TABLE terminal_sessions ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await
        .ok();
        // Backfill sort_order for pre-migration rows so newest-first ordering
        // is preserved without user intervention. Uses -epoch-ms so lower
        // values sort first. Only touches rows that still have the default 0.
        sqlx::query(
            "UPDATE terminal_sessions
             SET sort_order = -CAST((julianday(started_at) - 2440587.5) * 86400000 AS INTEGER)
             WHERE sort_order = 0",
        )
        .execute(&self.pool)
        .await
        .ok();

        // Per-event stream: raw input/output/resize bytes for replay.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS terminal_events (
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                ts_us INTEGER NOT NULL,
                direction TEXT NOT NULL,
                payload BLOB NOT NULL,
                PRIMARY KEY (session_id, seq),
                FOREIGN KEY (session_id) REFERENCES terminal_sessions(id)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_terminal_events_session_seq
             ON terminal_events(session_id, seq)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_terminal_sessions_status_started
             ON terminal_sessions(status, started_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // Index supporting the new list ordering (status + sort_order).
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_terminal_sessions_sort_order
             ON terminal_sessions(sort_order)",
        )
        .execute(&self.pool)
        .await?;

        // Indexes for PostToolUse correlation and history queries
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_decision_log_agent_tool_resolved
             ON decision_log(agent_id, tool_name, resolved_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_decision_log_resolved_at
             ON decision_log(resolved_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // Unique index on tool_use_id for deduplication (NULL values excluded)
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_decision_log_tool_use_id
             ON decision_log(tool_use_id) WHERE tool_use_id IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;

        // ── Web UI auth tables ───────────────────────────────────────
        // Single-row password table (id always = 1); argon2id hash.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_password (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                argon2_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Per-device tokens. `token_hash` = sha256(raw token); raw token is
        // only ever shown once to the client at login time.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_devices (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                last_seen_at TEXT,
                last_ip TEXT,
                revoked_at TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        // WebAuthn credentials bound to a device. Cascade-deleted so revoking
        // a device cleans up its passkeys too.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_passkeys (
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES web_devices(id) ON DELETE CASCADE,
                public_key BLOB NOT NULL,
                sign_count INTEGER NOT NULL,
                transports TEXT,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        // Append-only audit log for login/enroll/revoke events.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at TEXT NOT NULL,
                event TEXT NOT NULL,
                device_id TEXT,
                ip TEXT,
                detail TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_web_audit_at ON web_audit(at DESC)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_web_devices_revoked ON web_devices(revoked_at)",
        )
        .execute(&self.pool)
        .await?;

        // Enable WAL mode and performance pragmas
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&self.pool)
            .await?;
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&self.pool)
            .await?;
        sqlx::query("PRAGMA cache_size = -64000")
            .execute(&self.pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout = 5000")
            .execute(&self.pool)
            .await?;
        sqlx::query("PRAGMA temp_store = MEMORY")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Persist a pending decision for crash recovery.
    pub async fn persist_pending(&self, req: &wisphive_protocol::DecisionRequest) -> Result<()> {
        // For events without tool_input (Stop, ConfigChange, etc.), store event_data instead
        let stored_input = if req.tool_input.is_null() {
            if let Some(ref data) = req.event_data {
                data.clone()
            } else {
                req.tool_input.clone()
            }
        } else {
            req.tool_input.clone()
        };

        sqlx::query(
            "INSERT OR REPLACE INTO pending_decisions (id, agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id, hook_event_name, terminal_session_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(req.id.to_string())
        .bind(&req.agent_id)
        .bind(serde_json::to_string(&req.agent_type)?)
        .bind(req.project.to_string_lossy().to_string())
        .bind(&req.tool_name)
        .bind(serde_json::to_string(&stored_input)?)
        .bind(req.timestamp.to_rfc3339())
        .bind(&req.tool_use_id)
        .bind(req.hook_event_name.to_string())
        .bind(req.terminal_session_id.map(|u| u.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a pending decision after resolution and log it.
    pub async fn resolve_pending(
        &self,
        id: uuid::Uuid,
        decision: wisphive_protocol::Decision,
    ) -> Result<()> {
        // Move from pending to log
        let row = sqlx::query_as::<_, PendingRowWithTerm>(
            "SELECT agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id, hook_event_name, terminal_session_id
             FROM pending_decisions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some((
            agent_id,
            agent_type,
            project,
            tool_name,
            tool_input,
            requested_at,
            tool_use_id,
            hook_event_name,
            terminal_session_id,
        )) = row
        {
            sqlx::query(
                "INSERT OR IGNORE INTO decision_log (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id, hook_event_name, terminal_session_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(agent_id)
            .bind(agent_type)
            .bind(project)
            .bind(tool_name)
            .bind(tool_input)
            .bind(serde_json::to_string(&decision)?)
            .bind(requested_at)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(tool_use_id)
            .bind(hook_event_name)
            .bind(terminal_session_id)
            .execute(&self.pool)
            .await?;
        }

        sqlx::query("DELETE FROM pending_decisions WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Query the decision history log.
    ///
    /// Returns entries in reverse chronological order (most recent first).
    /// If `agent_id` is provided, filters to that agent only.
    pub async fn query_history(
        &self,
        agent_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<wisphive_protocol::HistoryEntry>> {
        let rows: Vec<DecisionLogRow> =
            match agent_id {
                Some(aid) => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id
                         FROM decision_log WHERE agent_id = ? ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(aid)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id
                         FROM decision_log ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
            };

        Ok(rows_to_entries(rows))
    }

    /// Attach a tool result to a matching decision_log entry.
    ///
    /// If `tool_use_id` is provided, does an exact match. Otherwise falls back
    /// to fuzzy correlation by agent_id + tool_name + recency.
    pub async fn attach_tool_result(
        &self,
        agent_id: &str,
        tool_name: &str,
        tool_result: &serde_json::Value,
        tool_use_id: Option<&str>,
    ) -> Result<Option<uuid::Uuid>> {
        let result_json = serde_json::to_string(tool_result)?;

        // Try exact match by tool_use_id first
        if let Some(tui) = tool_use_id {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM decision_log
                 WHERE tool_use_id = ? AND tool_result IS NULL
                 LIMIT 1",
            )
            .bind(tui)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((id_str,)) = row {
                sqlx::query("UPDATE decision_log SET tool_result = ? WHERE id = ?")
                    .bind(&result_json)
                    .bind(&id_str)
                    .execute(&self.pool)
                    .await?;
                return Ok(id_str.parse().ok());
            }
        }

        // Fallback: fuzzy match by agent_id + tool_name + recency
        let cutoff = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM decision_log
             WHERE agent_id = ? AND tool_name = ? AND tool_result IS NULL
             AND resolved_at > ?
             ORDER BY resolved_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .bind(tool_name)
        .bind(&cutoff)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id_str,)) = row {
            sqlx::query("UPDATE decision_log SET tool_result = ? WHERE id = ?")
                .bind(&result_json)
                .bind(&id_str)
                .execute(&self.pool)
                .await?;
            Ok(id_str.parse().ok())
        } else {
            Ok(None)
        }
    }

    /// Search decision history with free-text query across tool_input, tool_result, and tool_name.
    pub async fn search_history(
        &self,
        search: &wisphive_protocol::HistorySearch,
    ) -> Result<Vec<wisphive_protocol::HistoryEntry>> {
        let limit = search.limit.unwrap_or(200);

        // Build WHERE clause dynamically
        let mut conditions = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(ref q) = search.query {
            conditions.push(
                "(tool_input LIKE '%' || ? || '%' OR tool_result LIKE '%' || ? || '%' OR tool_name LIKE '%' || ? || '%')"
                    .to_string(),
            );
            binds.push(q.clone());
            binds.push(q.clone());
            binds.push(q.clone());
        }
        if let Some(ref tool) = search.tool_name {
            conditions.push("tool_name = ?".to_string());
            binds.push(tool.clone());
        }
        if let Some(ref aid) = search.agent_id {
            conditions.push("agent_id = ?".to_string());
            binds.push(aid.clone());
        }

        let where_clause = if conditions.is_empty() {
            "1=1".to_string()
        } else {
            conditions.join(" AND ")
        };

        let sql = format!(
            "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id
             FROM decision_log WHERE {} ORDER BY resolved_at DESC LIMIT ?",
            where_clause
        );

        let mut query = sqlx::query_as::<_, DecisionLogRow>(&sql);
        for bind in &binds {
            query = query.bind(bind);
        }
        query = query.bind(limit);

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows_to_entries(rows))
    }

    /// Get the underlying pool for direct queries.
    /// Insert an auto-approved tool call directly into decision_log.
    /// Called by the event ingest task when processing events.jsonl.
    pub async fn log_auto_approved(&self, entry: &AutoApprovedEntry<'_>) -> Result<()> {
        // Generate a deterministic UUID so repeated reimports of the same event
        // hit the PRIMARY KEY conflict and are ignored. This fixes bug #58.
        // When tool_use_id is present, derive from it; otherwise hash the content.
        let id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            match entry.tool_use_id {
                Some(tui) => tui.hash(&mut hasher),
                None => {
                    entry.agent_id.hash(&mut hasher);
                    entry.tool_name.hash(&mut hasher);
                    entry.timestamp.hash(&mut hasher);
                    entry.tool_input.hash(&mut hasher);
                }
            }
            let hash = hasher.finish();
            let bytes = hash.to_le_bytes();
            // Build a UUID-shaped string from the hash (deterministic, not RFC 4122)
            uuid::Uuid::from_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], 0,
                0, 0, 0, 0, 0, 0, 0,
            ])
            .to_string()
        };
        sqlx::query(
            "INSERT OR IGNORE INTO decision_log
             (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, auto_approved, tool_use_id, hook_event_name)
             VALUES (?, ?, ?, ?, ?, ?, '\"approve\"', ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(entry.agent_id)
        .bind(entry.agent_type)
        .bind(entry.project)
        .bind(entry.tool_name)
        .bind(entry.tool_input)
        .bind(entry.timestamp)
        .bind(entry.timestamp)
        .bind(entry.tool_use_id)
        .bind(entry.hook_event_name.unwrap_or("PreToolUse"))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Archive old decision_log entries to JSONL and delete from SQLite.
    ///
    /// Two pruning strategies applied in order:
    /// 1. Age: entries older than `max_age_days` are archived and deleted.
    /// 2. Count: if rows still exceed `max_rows`, oldest are archived and deleted.
    ///
    /// Returns the number of rows archived.
    pub async fn archive_and_prune(
        &self,
        archive_path: &std::path::Path,
        max_rows: u64,
        max_age_days: u64,
    ) -> Result<u64> {
        let mut total_archived = 0u64;

        // Phase 1: Archive entries older than max_age_days
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::days(max_age_days as i64)).to_rfc3339();
        let old_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM decision_log WHERE resolved_at < ? ORDER BY resolved_at ASC",
        )
        .bind(&cutoff)
        .fetch_all(&self.pool)
        .await?;

        if !old_rows.is_empty() {
            total_archived += self.archive_rows_by_ids(&old_rows, archive_path).await?;
        }

        // Phase 2: If still over max_rows, trim oldest
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM decision_log")
            .fetch_one(&self.pool)
            .await?;

        if count.0 as u64 > max_rows {
            let excess = count.0 as u64 - max_rows;
            let excess_rows: Vec<(String,)> =
                sqlx::query_as("SELECT id FROM decision_log ORDER BY resolved_at ASC LIMIT ?")
                    .bind(excess as i64)
                    .fetch_all(&self.pool)
                    .await?;

            if !excess_rows.is_empty() {
                total_archived += self.archive_rows_by_ids(&excess_rows, archive_path).await?;
            }
        }

        // Reclaim disk space if we archived anything
        if total_archived > 0
            && let Err(e) = sqlx::query("VACUUM").execute(&self.pool).await
        {
            tracing::warn!("VACUUM after retention failed: {e}");
        }

        Ok(total_archived)
    }

    /// Archive specific rows to JSONL file and delete from SQLite.
    ///
    /// Processes in batches of 500 for efficiency. Rows are written to the
    /// archive file before being deleted, ensuring no data loss.
    async fn archive_rows_by_ids(
        &self,
        ids: &[(String,)],
        archive_path: &std::path::Path,
    ) -> Result<u64> {
        use std::io::Write;

        let mut archived = 0u64;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_path)?;

        for chunk in ids.chunks(500) {
            // Build placeholders for batch SELECT
            let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
            let select_sql = format!(
                "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, \
                 requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id \
                 FROM decision_log WHERE id IN ({})",
                placeholders.join(",")
            );

            let mut query = sqlx::query_as::<_, DecisionLogRow>(&select_sql);
            for (id,) in chunk {
                query = query.bind(id);
            }
            let rows = query.fetch_all(&self.pool).await?;

            // Write all rows to archive file
            for (
                id,
                agent_id,
                _agent_type,
                project,
                tool_name,
                tool_input,
                decision,
                requested_at,
                resolved_at,
                tool_result,
                tool_use_id,
                hook_event_name,
                terminal_session_id,
            ) in &rows
            {
                let entry = serde_json::json!({
                    "id": id,
                    "agent_id": agent_id,
                    "project": project,
                    "tool_name": tool_name,
                    "tool_input": serde_json::from_str::<serde_json::Value>(tool_input).unwrap_or(serde_json::Value::Null),
                    "decision": decision,
                    "requested_at": requested_at,
                    "resolved_at": resolved_at,
                    "tool_result": tool_result.as_deref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
                    "tool_use_id": tool_use_id,
                    "hook_event_name": hook_event_name,
                    "terminal_session_id": terminal_session_id,
                });
                let mut line = serde_json::to_string(&entry).unwrap_or_default();
                line.push('\n');
                file.write_all(line.as_bytes())?;
                archived += 1;
            }
            file.flush()?;

            // Batch delete after archive is flushed to disk
            let delete_sql = format!(
                "DELETE FROM decision_log WHERE id IN ({})",
                placeholders.join(",")
            );
            let mut delete_query = sqlx::query(&delete_sql);
            for (id,) in chunk {
                delete_query = delete_query.bind(id);
            }
            delete_query.execute(&self.pool).await?;
        }

        Ok(archived)
    }

    /// Query distinct sessions from decision_log with aggregated stats.
    pub async fn query_sessions(&self) -> Result<Vec<wisphive_protocol::SessionSummary>> {
        let rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT agent_id, agent_type, project,
                    MIN(requested_at) as first_seen,
                    MAX(resolved_at) as last_seen,
                    COUNT(*) as total_calls,
                    SUM(CASE WHEN decision = '\"approve\"' THEN 1 ELSE 0 END) as approved,
                    SUM(CASE WHEN decision = '\"deny\"' THEN 1 ELSE 0 END) as denied
             FROM decision_log
             GROUP BY agent_id
             ORDER BY last_seen DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(
                |(
                    agent_id,
                    agent_type,
                    project,
                    first_seen,
                    last_seen,
                    total,
                    approved,
                    denied,
                )| {
                    Some(wisphive_protocol::SessionSummary {
                        agent_id,
                        agent_type: serde_json::from_str(&agent_type).ok()?,
                        project: std::path::PathBuf::from(project),
                        first_seen: chrono::DateTime::parse_from_rfc3339(&first_seen)
                            .ok()?
                            .with_timezone(&chrono::Utc),
                        last_seen: chrono::DateTime::parse_from_rfc3339(&last_seen)
                            .ok()?
                            .with_timezone(&chrono::Utc),
                        total_calls: total as u32,
                        approved: approved as u32,
                        denied: denied as u32,
                        is_live: false,
                        pending_count: 0,
                    })
                },
            )
            .collect())
    }

    /// Query distinct projects from decision_log with aggregated stats.
    pub async fn query_projects(&self) -> Result<Vec<wisphive_protocol::ProjectSummary>> {
        let rows: Vec<(String, String, String, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT project,
                    MIN(requested_at) as first_seen,
                    MAX(resolved_at) as last_seen,
                    COUNT(*) as total_calls,
                    SUM(CASE WHEN decision = '\"approve\"' THEN 1 ELSE 0 END) as approved,
                    SUM(CASE WHEN decision = '\"deny\"' THEN 1 ELSE 0 END) as denied,
                    COUNT(DISTINCT agent_id) as agent_count
             FROM decision_log
             GROUP BY project
             ORDER BY last_seen DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(
                |(project, first_seen, last_seen, total, approved, denied, agent_count)| {
                    Some(wisphive_protocol::ProjectSummary {
                        project: std::path::PathBuf::from(project),
                        first_seen: chrono::DateTime::parse_from_rfc3339(&first_seen)
                            .ok()?
                            .with_timezone(&chrono::Utc),
                        last_seen: chrono::DateTime::parse_from_rfc3339(&last_seen)
                            .ok()?
                            .with_timezone(&chrono::Utc),
                        total_calls: total as u32,
                        approved: approved as u32,
                        denied: denied as u32,
                        agent_count: agent_count as u32,
                        pending_count: 0,
                        has_live_agents: false,
                    })
                },
            )
            .collect())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ── Web UI auth helpers ───────────────────────────────────────
    //
    // All helpers return `WebAuthResult<T>` (not `anyhow::Result`) so auth
    // callers can distinguish NotFound / Revoked / Duplicate / Db without
    // string-matching on error messages.

    /// Upsert the single-row web password hash.
    pub async fn set_web_password(&self, argon2_hash: &str) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_password (id, argon2_hash, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET argon2_hash = excluded.argon2_hash, updated_at = excluded.updated_at",
        )
        .bind(argon2_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Atomic first-set: returns `true` iff no password existed before this
    /// call. The onboarding endpoint uses this instead of check-then-upsert
    /// so two concurrent first-run set-password requests can't both
    /// "succeed" — the second race-loser sees `false` and gets a 409.
    pub async fn try_set_initial_web_password(&self, argon2_hash: &str) -> WebAuthResult<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO web_password (id, argon2_hash, updated_at) VALUES (1, ?, ?)",
        )
        .bind(argon2_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }

    /// Fetch the stored web password hash, if one has been set.
    pub async fn get_web_password_hash(&self) -> WebAuthResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT argon2_hash FROM web_password WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(WebAuthError::from_sqlx)?;
        Ok(row.map(|(h,)| h))
    }

    /// Wipe the password + all devices + passkeys (reset). The audit rows
    /// stay so the operator can see the reset event.
    ///
    /// Passkey rows would be reaped by the `ON DELETE CASCADE` on
    /// `web_passkeys.device_id` once `web_devices` is deleted, but we delete
    /// them explicitly first so the transaction is resilient to an operator
    /// running against an older DB where `foreign_keys=OFF` happened to be
    /// the default.
    pub async fn reset_web_password(&self) -> WebAuthResult<()> {
        let mut tx = self.pool.begin().await.map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_passkeys")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_devices")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_password")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        tx.commit().await.map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Record a new device token binding.
    ///
    /// INVARIANT — caller MUST pass:
    ///   - `id`: a UUIDv4 string (never reused across a reset)
    ///   - `token_hash`: hex-encoded sha256 of a raw bearer ≥32 random bytes
    ///     (base64url-encoded). The raw token must never reach this crate —
    ///     storing a hash means a `wisphive.db` leak does not yield usable
    ///     credentials.
    ///
    /// Returns `Duplicate` if either `id` or `token_hash` already exists.
    pub async fn insert_web_device(
        &self,
        id: &str,
        name: &str,
        token_hash: &str,
    ) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_devices (id, name, token_hash, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(token_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Find a non-revoked device by its token hash. Also returns the device
    /// name so callers can populate the request context.
    ///
    /// Relies on the `UNIQUE` constraint on `token_hash` for the "at most
    /// one match" invariant; `LIMIT 1` is a belt-and-suspenders guard.
    pub async fn find_web_device_by_token_hash(
        &self,
        token_hash: &str,
    ) -> WebAuthResult<Option<WebDeviceRow>> {
        let row: Option<WebDeviceActiveRow> = sqlx::query_as(
            "SELECT id, name, created_at, last_seen_at, last_ip
             FROM web_devices
             WHERE token_hash = ? AND revoked_at IS NULL
             LIMIT 1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(row.map(
            |(id, name, created_at, last_seen_at, last_ip)| WebDeviceRow {
                id,
                name,
                created_at,
                last_seen_at,
                last_ip,
                revoked_at: None,
            },
        ))
    }

    /// Flip `revoked_at` on a device, idempotently. A second call is a
    /// no-op because the WHERE clause filters already-revoked rows.
    pub async fn revoke_web_device(&self, id: &str) -> WebAuthResult<()> {
        sqlx::query(
            "UPDATE web_devices SET revoked_at = ?
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Record that we've just served a request on behalf of `device_id`.
    /// Best-effort: callers should fire-and-forget.
    ///
    /// Only touches non-revoked devices so post-revocation forensics stay
    /// clean (a revoked device's `last_seen_at` is frozen at the moment of
    /// its last legitimate use).
    pub async fn touch_web_device(&self, id: &str, ip: Option<&str>) -> WebAuthResult<()> {
        sqlx::query(
            "UPDATE web_devices SET last_seen_at = ?, last_ip = ?
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(ip)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// List all devices, newest first. Includes revoked so the UI can show
    /// history.
    pub async fn list_web_devices(&self) -> WebAuthResult<Vec<WebDeviceRow>> {
        let rows: Vec<WebDeviceFullRow> = sqlx::query_as(
            "SELECT id, name, created_at, last_seen_at, last_ip, revoked_at
                 FROM web_devices
                 ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, name, created_at, last_seen_at, last_ip, revoked_at)| WebDeviceRow {
                    id,
                    name,
                    created_at,
                    last_seen_at,
                    last_ip,
                    revoked_at,
                },
            )
            .collect())
    }

    /// Persist a newly enrolled passkey. Returns `Duplicate` if the
    /// credential id is already enrolled.
    pub async fn insert_web_passkey(
        &self,
        id: &str,
        device_id: &str,
        public_key: &[u8],
        sign_count: i64,
        transports_json: Option<&str>,
    ) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_passkeys (id, device_id, public_key, sign_count, transports, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(device_id)
        .bind(public_key)
        .bind(sign_count)
        .bind(transports_json)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// List all passkeys bound to a given device.
    pub async fn list_web_passkeys_for_device(
        &self,
        device_id: &str,
    ) -> WebAuthResult<Vec<WebPasskeyRow>> {
        let rows: Vec<WebPasskeyRowRaw> = sqlx::query_as(
            "SELECT id, device_id, public_key, sign_count, transports, created_at, last_used_at
                 FROM web_passkeys
                 WHERE device_id = ?
                 ORDER BY created_at DESC",
        )
        .bind(device_id)
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, device_id, public_key, sign_count, transports, created_at, last_used_at)| {
                    WebPasskeyRow {
                        id,
                        device_id,
                        public_key,
                        sign_count,
                        transports,
                        created_at,
                        last_used_at,
                    }
                },
            )
            .collect())
    }

    /// Append a row to the audit log. `detail` is typically JSON; anything
    /// over 4KB is truncated so a LAN attacker hammering /login cannot
    /// inflate the DB with unbounded attacker-controlled payloads.
    pub async fn append_web_audit(
        &self,
        event: &str,
        device_id: Option<&str>,
        ip: Option<&str>,
        detail: Option<&str>,
    ) -> WebAuthResult<()> {
        const MAX_DETAIL: usize = 4096;
        let detail = detail.map(|d| {
            if d.len() > MAX_DETAIL {
                // Truncate at a char boundary to keep the row as valid UTF-8.
                let mut cut = MAX_DETAIL;
                while !d.is_char_boundary(cut) {
                    cut -= 1;
                }
                &d[..cut]
            } else {
                d
            }
        });
        sqlx::query(
            "INSERT INTO web_audit (at, event, device_id, ip, detail)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(event)
        .bind(device_id)
        .bind(ip)
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Query recent audit rows, newest first. Limit is clamped at 1000 so a
    /// misbehaving caller cannot force SQLite to materialize the whole
    /// table.
    pub async fn list_web_audit(&self, limit: u32) -> WebAuthResult<Vec<WebAuditRow>> {
        let clamped = limit.min(1000);
        let rows: Vec<WebAuditRowRaw> = sqlx::query_as(
            "SELECT id, at, event, device_id, ip, detail
                 FROM web_audit ORDER BY id DESC LIMIT ?",
        )
        .bind(clamped)
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|(id, at, event, device_id, ip, detail)| WebAuditRow {
                id,
                at,
                event,
                device_id,
                ip,
                detail,
            })
            .collect())
    }

    // ── Terminal session helpers ──────────────────────────────────

    /// Insert a new terminal session row.
    pub async fn create_terminal_session(&self, meta: &TerminalSessionMeta) -> Result<()> {
        let args_json = serde_json::to_string(&meta.args)?;
        sqlx::query(
            "INSERT INTO terminal_sessions (id, label, command, args, cwd, env_json, cols, rows, started_at, ended_at, exit_code, status, group_name, sort_order)
             VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(meta.id.to_string())
        .bind(&meta.label)
        .bind(&meta.command)
        .bind(args_json)
        .bind(meta.cwd.to_string_lossy().to_string())
        .bind(i64::from(meta.cols))
        .bind(i64::from(meta.rows))
        .bind(meta.started_at.to_rfc3339())
        .bind(meta.ended_at.map(|t| t.to_rfc3339()))
        .bind(meta.exit_code)
        .bind(meta.status.to_string())
        .bind(meta.group_name.as_deref())
        .bind(meta.sort_order)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Assign (or clear, when `group` is None) the group label for a session.
    pub async fn set_terminal_group(&self, id: uuid::Uuid, group: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET group_name = ? WHERE id = ?")
            .bind(group)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update a session's manual sort order.
    pub async fn set_terminal_sort_order(&self, id: uuid::Uuid, sort_order: i64) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET sort_order = ? WHERE id = ?")
            .bind(sort_order)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark a terminal session as finished and record its final status.
    pub async fn end_terminal_session(
        &self,
        id: uuid::Uuid,
        exit_code: Option<i32>,
        status: TerminalStatus,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE terminal_sessions
             SET ended_at = ?, exit_code = ?, status = ?
             WHERE id = ?",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(exit_code)
        .bind(status.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List all terminal sessions. Ordered by `sort_order` ASC (manual order,
    /// with a newest-first default baked in at creation), tiebroken by
    /// `started_at` DESC. The client is responsible for sectioning by status.
    pub async fn list_terminal_sessions(&self) -> Result<Vec<TerminalSessionMeta>> {
        type Row = (
            String,
            Option<String>,
            String,
            String,
            String,
            i64,
            i64,
            String,
            Option<String>,
            Option<i64>,
            String,
            Option<String>,
            i64,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, label, command, args, cwd, cols, rows, started_at, ended_at, exit_code, status, group_name, sort_order
             FROM terminal_sessions
             ORDER BY sort_order ASC, started_at DESC
             LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (
            id,
            label,
            command,
            args_json,
            cwd,
            cols,
            rows_,
            started_at,
            ended_at,
            exit_code,
            status,
            group_name,
            sort_order,
        ) in rows
        {
            let Ok(id) = uuid::Uuid::parse_str(&id) else {
                continue;
            };
            let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
            let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&started_at) else {
                continue;
            };
            let ended_at = ended_at
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&chrono::Utc));
            let Ok(status) = status.parse::<TerminalStatus>() else {
                continue;
            };
            out.push(TerminalSessionMeta {
                id,
                label,
                command,
                args,
                cwd: std::path::PathBuf::from(cwd),
                cols: cols as u16,
                rows: rows_ as u16,
                started_at: started_at.with_timezone(&chrono::Utc),
                ended_at,
                exit_code: exit_code.map(|c| c as i32),
                status,
                group_name,
                sort_order,
            });
        }
        Ok(out)
    }

    /// Look up a single terminal session by ID.
    pub async fn get_terminal_session(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<TerminalSessionMeta>> {
        // Tiny wrapper: filter list_terminal_sessions by id. For 500-row
        // cap that is cheap; avoids a duplicate query/hydration path.
        Ok(self
            .list_terminal_sessions()
            .await?
            .into_iter()
            .find(|m| m.id == id))
    }

    /// Insert a batch of terminal events in a single transaction.
    ///
    /// `rows` is `(session_id, seq, ts_us, direction, payload)`.
    pub async fn insert_terminal_events_batch(
        &self,
        rows: &[(uuid::Uuid, u64, i64, TerminalDirection, Vec<u8>)],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for (session_id, seq, ts_us, direction, payload) in rows {
            sqlx::query(
                "INSERT OR IGNORE INTO terminal_events (session_id, seq, ts_us, direction, payload)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(session_id.to_string())
            .bind(*seq as i64)
            .bind(*ts_us)
            .bind(direction.to_string())
            .bind(payload.as_slice())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Stream events for replay. Returns `(seq, ts_us, direction, payload)`.
    pub async fn replay_terminal_events(
        &self,
        id: uuid::Uuid,
        from_seq: Option<u64>,
    ) -> Result<Vec<(u64, i64, TerminalDirection, Vec<u8>)>> {
        let rows: Vec<(i64, i64, String, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, ts_us, direction, payload
             FROM terminal_events
             WHERE session_id = ? AND seq >= ?
             ORDER BY seq ASC",
        )
        .bind(id.to_string())
        .bind(from_seq.unwrap_or(0) as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (seq, ts_us, dir, payload) in rows {
            let Ok(direction) = dir.parse::<TerminalDirection>() else {
                continue;
            };
            out.push((seq as u64, ts_us, direction, payload));
        }
        Ok(out)
    }

    /// Mark any sessions still flagged 'running' as orphaned. Called on daemon
    /// startup — a running session across a restart has no live PTY behind it.
    pub async fn mark_running_terminals_orphaned(&self) -> Result<()> {
        sqlx::query(
            "UPDATE terminal_sessions
             SET status = 'orphaned', ended_at = COALESCE(ended_at, ?)
             WHERE status = 'running'",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete terminal events older than the retention cutoff for sessions
    /// that have already ended. Metadata rows are preserved.
    pub async fn prune_terminal_events(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64> {
        let res = sqlx::query(
            "DELETE FROM terminal_events
             WHERE session_id IN (
                 SELECT id FROM terminal_sessions
                 WHERE ended_at IS NOT NULL AND ended_at < ?
             )",
        )
        .bind(cutoff.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
}

/// Convert raw SQL rows to HistoryEntry structs.
fn rows_to_entries(rows: Vec<DecisionLogRow>) -> Vec<wisphive_protocol::HistoryEntry> {
    rows.into_iter()
        .filter_map(
            |(
                id,
                agent_id,
                agent_type,
                project,
                tool_name,
                tool_input,
                decision,
                requested_at,
                resolved_at,
                tool_result,
                tool_use_id,
                hook_event_name,
                terminal_session_id,
            )| {
                Some(wisphive_protocol::HistoryEntry {
                    id: id.parse().ok()?,
                    agent_id,
                    agent_type: serde_json::from_str(&agent_type).ok()?,
                    project: std::path::PathBuf::from(project),
                    tool_name,
                    tool_input: serde_json::from_str(&tool_input)
                        .unwrap_or(serde_json::Value::Null),
                    decision: serde_json::from_str(&decision).ok()?,
                    requested_at: chrono::DateTime::parse_from_rfc3339(&requested_at)
                        .ok()?
                        .with_timezone(&chrono::Utc),
                    resolved_at: chrono::DateTime::parse_from_rfc3339(&resolved_at)
                        .ok()?
                        .with_timezone(&chrono::Utc),
                    tool_result: tool_result.and_then(|s| serde_json::from_str(&s).ok()),
                    tool_use_id,
                    hook_event_name,
                    terminal_session_id: terminal_session_id
                        .as_deref()
                        .and_then(|s| uuid::Uuid::parse_str(s).ok()),
                })
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wisphive_protocol::{AgentType, Decision, DecisionRequest, HookEventType};

    /// Create an in-memory StateDb for testing.
    async fn test_db() -> StateDb {
        StateDb::open(":memory:").await.unwrap()
    }

    fn make_request(tool: &str, agent_id: &str, project: &str) -> DecisionRequest {
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

    fn make_request_with_tool_use_id(
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

    /// Shorthand for tests: call log_auto_approved with positional args.
    #[allow(clippy::too_many_arguments)]
    async fn log_auto(
        db: &StateDb,
        agent_id: &str,
        agent_type: &str,
        project: &str,
        tool_name: &str,
        tool_input: &str,
        timestamp: &str,
        tool_use_id: Option<&str>,
        hook_event_name: Option<&str>,
    ) {
        db.log_auto_approved(&AutoApprovedEntry {
            agent_id,
            agent_type,
            project,
            tool_name,
            tool_input,
            timestamp,
            tool_use_id,
            hook_event_name,
        })
        .await
        .unwrap();
    }

    // ════════════════════════════════════════════════════════════
    // persist_pending + resolve_pending
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn persist_and_resolve_pending() {
        let db = test_db().await;
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;

        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(id, Decision::Approve).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "Bash");
        assert_eq!(history[0].decision, Decision::Approve);
    }

    #[tokio::test]
    async fn resolve_pending_removes_from_pending() {
        let db = test_db().await;
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;

        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(id, Decision::Deny).await.unwrap();

        // Resolving again should be a no-op (pending row already deleted)
        db.resolve_pending(id, Decision::Approve).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        // Should still be just 1 entry (the deny), not 2
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].decision, Decision::Deny);
    }

    #[tokio::test]
    async fn resolve_nonexistent_pending_is_noop() {
        let db = test_db().await;
        let fake_id = uuid::Uuid::new_v4();
        // Should not error — just silently does nothing
        db.resolve_pending(fake_id, Decision::Approve)
            .await
            .unwrap();
        let history = db.query_history(None, 10).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn persist_pending_with_tool_use_id() {
        let db = test_db().await;
        let req = make_request_with_tool_use_id("Bash", "cc-1", "tui-123");
        let id = req.id;

        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(id, Decision::Approve).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_use_id, Some("tui-123".to_string()));
    }

    // ════════════════════════════════════════════════════════════
    // query_history
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn query_history_empty_db() {
        let db = test_db().await;
        let history = db.query_history(None, 10).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn query_history_filters_by_agent_id() {
        let db = test_db().await;

        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Edit", "cc-2", "/rpg");
        let r3 = make_request("Write", "cc-1", "/muse");

        db.persist_pending(&r1).await.unwrap();
        db.resolve_pending(r1.id, Decision::Approve).await.unwrap();
        db.persist_pending(&r2).await.unwrap();
        db.resolve_pending(r2.id, Decision::Deny).await.unwrap();
        db.persist_pending(&r3).await.unwrap();
        db.resolve_pending(r3.id, Decision::Approve).await.unwrap();

        let all = db.query_history(None, 10).await.unwrap();
        assert_eq!(all.len(), 3);

        let cc1 = db.query_history(Some("cc-1"), 10).await.unwrap();
        assert_eq!(cc1.len(), 2);
        assert!(cc1.iter().all(|e| e.agent_id == "cc-1"));

        let cc2 = db.query_history(Some("cc-2"), 10).await.unwrap();
        assert_eq!(cc2.len(), 1);
        assert_eq!(cc2[0].tool_name, "Edit");
    }

    #[tokio::test]
    async fn query_history_respects_limit() {
        let db = test_db().await;

        for i in 0..5 {
            let r = make_request(&format!("Tool{i}"), "cc-1", "/muse");
            db.persist_pending(&r).await.unwrap();
            db.resolve_pending(r.id, Decision::Approve).await.unwrap();
        }

        let limited = db.query_history(None, 3).await.unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[tokio::test]
    async fn query_history_reverse_chronological() {
        let db = test_db().await;

        let r1 = make_request("First", "cc-1", "/muse");
        db.persist_pending(&r1).await.unwrap();
        db.resolve_pending(r1.id, Decision::Approve).await.unwrap();

        let r2 = make_request("Second", "cc-1", "/muse");
        db.persist_pending(&r2).await.unwrap();
        db.resolve_pending(r2.id, Decision::Approve).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history[0].tool_name, "Second"); // most recent first
        assert_eq!(history[1].tool_name, "First");
    }

    // ════════════════════════════════════════════════════════════
    // log_auto_approved
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn log_auto_approved_creates_entry() {
        let db = test_db().await;
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Read",
            "{}",
            "2024-01-01T00:00:00Z",
            Some("tui-1"),
            Some("PreToolUse"),
        )
        .await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "Read");
        assert_eq!(history[0].decision, Decision::Approve);
    }

    #[tokio::test]
    async fn log_auto_approved_dedup_with_tool_use_id() {
        let db = test_db().await;

        // Insert same event twice with same tool_use_id
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Read",
            "{}",
            "2024-01-01T00:00:00Z",
            Some("tui-1"),
            Some("PreToolUse"),
        )
        .await;
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Read",
            "{}",
            "2024-01-01T00:00:00Z",
            Some("tui-1"),
            Some("PreToolUse"),
        )
        .await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(
            history.len(),
            1,
            "duplicate with same tool_use_id should be ignored"
        );
    }

    /// Fixed #58: Events without tool_use_id are now deduplicated via
    /// deterministic content-hashed IDs in log_auto_approved().
    #[tokio::test]
    async fn log_auto_approved_dedup_without_tool_use_id() {
        let db = test_db().await;

        // Insert same event twice with NO tool_use_id
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Read",
            "{}",
            "2024-01-01T00:00:00Z",
            None,
            Some("PreToolUse"),
        )
        .await;
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Read",
            "{}",
            "2024-01-01T00:00:00Z",
            None,
            Some("PreToolUse"),
        )
        .await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(
            history.len(),
            1,
            "deterministic IDs should deduplicate events without tool_use_id"
        );
    }

    // ════════════════════════════════════════════════════════════
    // attach_tool_result
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn attach_tool_result_by_tool_use_id() {
        let db = test_db().await;
        let req = make_request_with_tool_use_id("Bash", "cc-1", "tui-456");
        let id = req.id;
        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(id, Decision::Approve).await.unwrap();

        let result = serde_json::json!({"output": "build succeeded"});
        let matched = db
            .attach_tool_result("cc-1", "Bash", &result, Some("tui-456"))
            .await
            .unwrap();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap(), id);

        let history = db.query_history(None, 10).await.unwrap();
        assert!(history[0].tool_result.is_some());
    }

    #[tokio::test]
    async fn attach_tool_result_fuzzy_fallback() {
        let db = test_db().await;
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(id, Decision::Approve).await.unwrap();

        let result = serde_json::json!({"output": "ok"});
        // No tool_use_id → fuzzy match by agent_id + tool_name + recency
        let matched = db
            .attach_tool_result("cc-1", "Bash", &result, None)
            .await
            .unwrap();
        assert!(matched.is_some());
    }

    #[tokio::test]
    async fn attach_tool_result_no_match() {
        let db = test_db().await;
        let result = serde_json::json!({"output": "orphan"});
        let matched = db
            .attach_tool_result("cc-99", "Bash", &result, None)
            .await
            .unwrap();
        assert!(matched.is_none());
    }

    #[tokio::test]
    async fn attach_tool_result_does_not_overwrite() {
        let db = test_db().await;
        let req = make_request_with_tool_use_id("Bash", "cc-1", "tui-789");
        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(req.id, Decision::Approve).await.unwrap();

        let r1 = serde_json::json!({"output": "first"});
        db.attach_tool_result("cc-1", "Bash", &r1, Some("tui-789"))
            .await
            .unwrap();

        // Second attach to same tool_use_id should find no match (already has result)
        let r2 = serde_json::json!({"output": "second"});
        let matched = db
            .attach_tool_result("cc-1", "Bash", &r2, Some("tui-789"))
            .await
            .unwrap();
        assert!(
            matched.is_none(),
            "should not overwrite existing tool_result"
        );
    }

    // ════════════════════════════════════════════════════════════
    // search_history
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn search_history_by_query() {
        let db = test_db().await;

        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Bash",
            "{\"command\":\"cargo build\"}",
            "2024-01-01T00:00:00Z",
            Some("a"),
            None,
        )
        .await;
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Edit",
            "{\"file\":\"main.rs\"}",
            "2024-01-01T00:01:00Z",
            Some("b"),
            None,
        )
        .await;

        let search = wisphive_protocol::HistorySearch {
            query: Some("cargo".into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "Bash");
    }

    #[tokio::test]
    async fn search_history_by_tool_name() {
        let db = test_db().await;

        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Bash",
            "{}",
            "2024-01-01T00:00:00Z",
            Some("a"),
            None,
        )
        .await;
        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Edit",
            "{}",
            "2024-01-01T00:01:00Z",
            Some("b"),
            None,
        )
        .await;

        let search = wisphive_protocol::HistorySearch {
            tool_name: Some("Edit".into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "Edit");
    }

    #[tokio::test]
    async fn search_history_by_agent_id() {
        let db = test_db().await;

        log_auto(
            &db,
            "cc-1",
            "\"claude_code\"",
            "/muse",
            "Bash",
            "{}",
            "2024-01-01T00:00:00Z",
            Some("a"),
            None,
        )
        .await;
        log_auto(
            &db,
            "cc-2",
            "\"claude_code\"",
            "/rpg",
            "Bash",
            "{}",
            "2024-01-01T00:01:00Z",
            Some("b"),
            None,
        )
        .await;

        let search = wisphive_protocol::HistorySearch {
            agent_id: Some("cc-2".into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "cc-2");
    }

    #[tokio::test]
    async fn search_history_empty_result() {
        let db = test_db().await;
        let search = wisphive_protocol::HistorySearch {
            query: Some("nonexistent".into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert!(results.is_empty());
    }

    // ════════════════════════════════════════════════════════════
    // query_sessions
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn query_sessions_empty() {
        let db = test_db().await;
        let sessions = db.query_sessions().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn query_sessions_aggregates_by_agent() {
        let db = test_db().await;

        // Two approves for cc-1, one deny for cc-2
        let r1 = make_request("Bash", "cc-1", "/muse");
        db.persist_pending(&r1).await.unwrap();
        db.resolve_pending(r1.id, Decision::Approve).await.unwrap();

        let r2 = make_request("Edit", "cc-1", "/muse");
        db.persist_pending(&r2).await.unwrap();
        db.resolve_pending(r2.id, Decision::Approve).await.unwrap();

        let r3 = make_request("Bash", "cc-2", "/rpg");
        db.persist_pending(&r3).await.unwrap();
        db.resolve_pending(r3.id, Decision::Deny).await.unwrap();

        let sessions = db.query_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);

        let s1 = sessions.iter().find(|s| s.agent_id == "cc-1").unwrap();
        assert_eq!(s1.total_calls, 2);
        assert_eq!(s1.approved, 2);
        assert_eq!(s1.denied, 0);

        let s2 = sessions.iter().find(|s| s.agent_id == "cc-2").unwrap();
        assert_eq!(s2.total_calls, 1);
        assert_eq!(s2.approved, 0);
        assert_eq!(s2.denied, 1);
    }

    // ════════════════════════════════════════════════════════════
    // query_projects
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn query_projects_empty() {
        let db = test_db().await;
        let projects = db.query_projects().await.unwrap();
        assert!(projects.is_empty());
    }

    #[tokio::test]
    async fn query_projects_aggregates_by_project() {
        let db = test_db().await;

        let r1 = make_request("Bash", "cc-1", "/muse");
        db.persist_pending(&r1).await.unwrap();
        db.resolve_pending(r1.id, Decision::Approve).await.unwrap();

        let r2 = make_request("Edit", "cc-2", "/muse");
        db.persist_pending(&r2).await.unwrap();
        db.resolve_pending(r2.id, Decision::Deny).await.unwrap();

        let r3 = make_request("Bash", "cc-3", "/rpg");
        db.persist_pending(&r3).await.unwrap();
        db.resolve_pending(r3.id, Decision::Approve).await.unwrap();

        let projects = db.query_projects().await.unwrap();
        assert_eq!(projects.len(), 2);

        let muse = projects
            .iter()
            .find(|p| p.project == std::path::Path::new("/muse"))
            .unwrap();
        assert_eq!(muse.total_calls, 2);
        assert_eq!(muse.agent_count, 2);
        assert_eq!(muse.approved, 1);
        assert_eq!(muse.denied, 1);

        let rpg = projects
            .iter()
            .find(|p| p.project == std::path::Path::new("/rpg"))
            .unwrap();
        assert_eq!(rpg.total_calls, 1);
        assert_eq!(rpg.agent_count, 1);
    }

    // ════════════════════════════════════════════════════════════
    // archive_and_prune
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn archive_prune_by_max_rows() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("archive.jsonl");

        // Insert 5 entries
        for i in 0..5 {
            let r = make_request(&format!("Tool{i}"), "cc-1", "/muse");
            db.persist_pending(&r).await.unwrap();
            db.resolve_pending(r.id, Decision::Approve).await.unwrap();
        }

        // Prune to max 3 rows
        let archived = db.archive_and_prune(&archive_path, 3, 365).await.unwrap();
        assert_eq!(archived, 2, "should archive 2 oldest entries");

        let remaining = db.query_history(None, 100).await.unwrap();
        assert_eq!(remaining.len(), 3);

        // Verify archive file was written
        let archive_content = std::fs::read_to_string(&archive_path).unwrap();
        let lines: Vec<&str> = archive_content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[tokio::test]
    async fn archive_prune_empty_db_is_noop() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("archive.jsonl");

        let archived = db.archive_and_prune(&archive_path, 100, 365).await.unwrap();
        assert_eq!(archived, 0);
        assert!(!archive_path.exists());
    }

    // ════════════════════════════════════════════════════════════
    // Terminal sessions
    // ════════════════════════════════════════════════════════════

    fn make_term_meta(id: uuid::Uuid) -> TerminalSessionMeta {
        TerminalSessionMeta {
            id,
            label: Some("main".into()),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            cwd: std::path::PathBuf::from("/tmp"),
            cols: 80,
            rows: 24,
            started_at: chrono::Utc::now(),
            ended_at: None,
            exit_code: None,
            status: TerminalStatus::Running,
            group_name: None,
            sort_order: 0,
        }
    }

    #[tokio::test]
    async fn terminal_session_create_and_list() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id))
            .await
            .unwrap();

        let list = db.list_terminal_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].command, "/bin/sh");
        assert_eq!(list[0].args, vec!["-c".to_string(), "echo hi".into()]);
        assert_eq!(list[0].status, TerminalStatus::Running);
    }

    #[tokio::test]
    async fn terminal_session_end_sets_fields() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id))
            .await
            .unwrap();
        db.end_terminal_session(id, Some(0), TerminalStatus::Exited)
            .await
            .unwrap();

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Exited);
        assert_eq!(got.exit_code, Some(0));
        assert!(got.ended_at.is_some());
    }

    #[tokio::test]
    async fn terminal_events_batch_and_replay_preserve_order_and_bytes() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id))
            .await
            .unwrap();

        let rows = vec![
            (
                id,
                1u64,
                100i64,
                TerminalDirection::Output,
                b"hello\n".to_vec(),
            ),
            (id, 2, 200, TerminalDirection::Input, b"yes\r".to_vec()),
            (
                id,
                3,
                300,
                TerminalDirection::Output,
                vec![0x1b, b'[', b'3', b'1', b'm'],
            ),
        ];
        db.insert_terminal_events_batch(&rows).await.unwrap();

        let replayed = db.replay_terminal_events(id, None).await.unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[0].2, TerminalDirection::Output);
        assert_eq!(replayed[0].3, b"hello\n");
        assert_eq!(replayed[2].3, vec![0x1b, b'[', b'3', b'1', b'm']);

        let from_two = db.replay_terminal_events(id, Some(2)).await.unwrap();
        assert_eq!(from_two.len(), 2);
        assert_eq!(from_two[0].0, 2);
    }

    #[tokio::test]
    async fn terminal_group_and_sort_order_round_trip() {
        let db = test_db().await;
        // Three sessions with distinct ids. Leave group/sort_order at defaults.
        let ids: Vec<uuid::Uuid> = (0..3).map(|_| uuid::Uuid::new_v4()).collect();
        for id in &ids {
            db.create_terminal_session(&make_term_meta(*id))
                .await
                .unwrap();
        }

        // Assign the first two to a group, reorder them.
        db.set_terminal_group(ids[0], Some("frontend"))
            .await
            .unwrap();
        db.set_terminal_group(ids[1], Some("frontend"))
            .await
            .unwrap();
        db.set_terminal_sort_order(ids[0], 200).await.unwrap();
        db.set_terminal_sort_order(ids[1], 100).await.unwrap();
        db.set_terminal_sort_order(ids[2], 50).await.unwrap();

        let list = db.list_terminal_sessions().await.unwrap();
        // Ordered by sort_order ASC: ids[2] (50), ids[1] (100), ids[0] (200).
        assert_eq!(list[0].id, ids[2]);
        assert_eq!(list[0].group_name, None);
        assert_eq!(list[0].sort_order, 50);
        assert_eq!(list[1].id, ids[1]);
        assert_eq!(list[1].group_name.as_deref(), Some("frontend"));
        assert_eq!(list[2].id, ids[0]);
        assert_eq!(list[2].group_name.as_deref(), Some("frontend"));

        // Clearing the group (None) removes the label.
        db.set_terminal_group(ids[0], None).await.unwrap();
        let after = db.list_terminal_sessions().await.unwrap();
        let found = after.iter().find(|m| m.id == ids[0]).unwrap();
        assert_eq!(found.group_name, None);
    }

    #[tokio::test]
    async fn mark_running_orphaned_on_startup() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id))
            .await
            .unwrap();
        // Directly invoke the sweeper (also runs inside StateDb::open).
        db.mark_running_terminals_orphaned().await.unwrap();
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Orphaned);
        assert!(got.ended_at.is_some());
    }

    /// Regression (itr#215 sec#5): `open_client` MUST NOT run the
    /// orphan-sweeper, otherwise CLI admin commands running against a
    /// shared DB will corrupt a live daemon's terminal sessions by
    /// flipping `running` rows to `orphaned` on every invocation.
    ///
    /// The test uses a file-backed DB because `:memory:` opens create a
    /// fresh DB per connection — `open_client` and our first test handle
    /// would see different data and the test would false-pass.
    #[tokio::test]
    async fn open_client_does_not_orphan_running_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let path_s = path.to_str().unwrap();

        // Simulate the daemon creating a running session.
        let daemon_db = StateDb::open(path_s).await.unwrap();
        let id = uuid::Uuid::new_v4();
        daemon_db
            .create_terminal_session(&make_term_meta(id))
            .await
            .unwrap();

        // CLI client opens the same DB — must NOT flip running → orphaned.
        let _client_db = StateDb::open_client(path_s).await.unwrap();

        let got = daemon_db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(
            got.status,
            TerminalStatus::Running,
            "open_client orphaned a running session — CLI would corrupt daemon state"
        );
        assert!(got.ended_at.is_none());
    }

    // ════════════════════════════════════════════════════════════
    // Web auth helpers
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn web_password_set_get_and_reset() {
        let db = test_db().await;
        assert!(db.get_web_password_hash().await.unwrap().is_none());

        db.set_web_password("$argon2id$hash1").await.unwrap();
        assert_eq!(
            db.get_web_password_hash().await.unwrap().as_deref(),
            Some("$argon2id$hash1")
        );

        // Upsert overwrites.
        db.set_web_password("$argon2id$hash2").await.unwrap();
        assert_eq!(
            db.get_web_password_hash().await.unwrap().as_deref(),
            Some("$argon2id$hash2")
        );

        // Reset cascades devices/passkeys and clears the password.
        db.insert_web_device("dev-1", "phone", "tokhash-1")
            .await
            .unwrap();
        db.insert_web_passkey("pk-1", "dev-1", b"fake-key", 0, None)
            .await
            .unwrap();
        db.reset_web_password().await.unwrap();
        assert!(db.get_web_password_hash().await.unwrap().is_none());
        assert!(db.list_web_devices().await.unwrap().is_empty());
        assert!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn web_device_insert_find_revoke_list() {
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.insert_web_device("dev-2", "laptop", "hash-2")
            .await
            .unwrap();

        let found = db
            .find_web_device_by_token_hash("hash-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "dev-1");
        assert_eq!(found.name, "phone");

        // Touching updates last_seen/last_ip (smoke test).
        db.touch_web_device("dev-1", Some("192.168.1.5"))
            .await
            .unwrap();

        // Listing returns both; order is newest-first so dev-2 comes first.
        let devices = db.list_web_devices().await.unwrap();
        assert_eq!(devices.len(), 2);

        // Revoking hides the device from token lookups and flips revoked_at.
        db.revoke_web_device("dev-1").await.unwrap();
        assert!(
            db.find_web_device_by_token_hash("hash-1")
                .await
                .unwrap()
                .is_none()
        );
        let rev = db.list_web_devices().await.unwrap();
        let dev1 = rev.iter().find(|d| d.id == "dev-1").unwrap();
        assert!(dev1.revoked_at.is_some());

        // Revoking twice is a no-op.
        db.revoke_web_device("dev-1").await.unwrap();
    }

    #[tokio::test]
    async fn web_passkey_insert_and_list_cascade_deletes() {
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.insert_web_passkey("pk-a", "dev-1", b"cose-a", 0, Some("[\"internal\"]"))
            .await
            .unwrap();
        db.insert_web_passkey("pk-b", "dev-1", b"cose-b", 0, None)
            .await
            .unwrap();

        let keys = db.list_web_passkeys_for_device("dev-1").await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|k| k.id == "pk-a"));
        assert!(keys.iter().any(|k| k.id == "pk-b"));
        assert_eq!(
            keys.iter().find(|k| k.id == "pk-a").unwrap().public_key,
            b"cose-a"
        );

        // ON DELETE CASCADE kicks in when the device row is removed (via reset).
        db.reset_web_password().await.unwrap();
        assert!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn web_device_token_hash_is_unique() {
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "same-hash")
            .await
            .unwrap();
        let err = db
            .insert_web_device("dev-2", "laptop", "same-hash")
            .await
            .expect_err("second device with same token_hash must fail");
        assert!(
            matches!(err, WebAuthError::Duplicate),
            "expected Duplicate, got {err:?}"
        );
    }

    #[tokio::test]
    async fn web_device_fk_cascade_drops_passkeys_when_device_row_is_deleted() {
        // Regression guard for the FK-off footgun: enabling
        // `foreign_keys=ON` at connect time makes the CASCADE actually fire.
        // If someone ever turns the pragma off the cascade test in
        // `web_passkey_insert_and_list_cascade_deletes` would still pass
        // (because reset_web_password deletes passkeys manually first), but
        // this one will not.
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.insert_web_passkey("pk-1", "dev-1", b"cose", 0, None)
            .await
            .unwrap();
        assert_eq!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .len(),
            1
        );

        sqlx::query("DELETE FROM web_devices WHERE id = ?")
            .bind("dev-1")
            .execute(db.pool())
            .await
            .unwrap();

        assert!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .is_empty(),
            "ON DELETE CASCADE must reap passkeys when foreign_keys=ON"
        );
    }

    #[tokio::test]
    async fn touch_web_device_ignores_revoked_rows() {
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.touch_web_device("dev-1", Some("10.0.0.1"))
            .await
            .unwrap();
        let before = db
            .list_web_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.id == "dev-1")
            .unwrap();
        assert_eq!(before.last_ip.as_deref(), Some("10.0.0.1"));

        db.revoke_web_device("dev-1").await.unwrap();
        // Attempt to touch after revocation — must be a silent no-op.
        db.touch_web_device("dev-1", Some("10.0.0.99"))
            .await
            .unwrap();

        let after = db
            .list_web_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.id == "dev-1")
            .unwrap();
        assert_eq!(
            after.last_ip.as_deref(),
            Some("10.0.0.1"),
            "revoked device's last_ip must be frozen"
        );
    }

    #[tokio::test]
    async fn web_audit_append_and_list_newest_first() {
        let db = test_db().await;
        db.append_web_audit(
            "login_failure",
            None,
            Some("1.2.3.4"),
            Some("{\"reason\":\"bad_pw\"}"),
        )
        .await
        .unwrap();
        db.append_web_audit("login_success", Some("dev-1"), Some("1.2.3.4"), None)
            .await
            .unwrap();

        let rows = db.list_web_audit(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first
        assert_eq!(rows[0].event, "login_success");
        assert_eq!(rows[0].device_id.as_deref(), Some("dev-1"));
        assert_eq!(rows[1].event, "login_failure");
        assert_eq!(rows[1].detail.as_deref(), Some("{\"reason\":\"bad_pw\"}"));
    }

    #[tokio::test]
    async fn terminal_session_id_persists_through_decision_log() {
        let db = test_db().await;
        let term_id = uuid::Uuid::new_v4();
        let mut req = make_request("Bash", "cc-1", "/proj");
        req.terminal_session_id = Some(term_id);
        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(req.id, Decision::Approve).await.unwrap();

        let history = db.query_history(Some("cc-1"), 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].terminal_session_id, Some(term_id));
    }
}
