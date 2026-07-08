use anyhow::Result;
use sqlx::SqlitePool;
use tracing::info;

use super::StateDb;

impl StateDb {
    /// Run schema migrations.
    pub(super) async fn migrate(&self) -> Result<()> {
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

        // Audit-trail columns (itr#397): which layer/rule resolved the
        // decision, and the config.json snapshot hash at decision time.
        sqlx::query("ALTER TABLE decision_log ADD COLUMN decided_by TEXT")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN config_hash TEXT")
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
        // Replay authorization (itr#98): `created_by` is the implicit owner
        // proof; `replay_acl` is an explicit per-session allowlist of resolver
        // labels (`human:tui` / `human:web:<device-id>`). Legacy rows have no
        // creator and an empty ACL, so replay fails closed unless access is
        // granted later.
        try_add_column(&self.pool, "terminal_sessions", "created_by", "TEXT").await?;
        try_add_column(
            &self.pool,
            "terminal_sessions",
            "replay_acl",
            "TEXT NOT NULL DEFAULT '[]'",
        )
        .await?;

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
        //
        // itr#311: `aaguid` + `rp_id` columns added. They live in the CREATE
        // TABLE block AND in the follow-on ALTER TABLE block below so a
        // fresh DB lands them at creation time while older DBs (created
        // pre-#311) get them via the idempotent ALTER. The migration is
        // load-bearing for profile-switch detection (see
        // `wisphive_web::auth_profile::scan_passkey_rp_id_drift`) — a row
        // with `rp_id = ''` is treated as "enrolled under an unknown
        // profile" and warned about at startup.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_passkeys (
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES web_devices(id) ON DELETE CASCADE,
                public_key BLOB NOT NULL,
                sign_count INTEGER NOT NULL,
                transports TEXT,
                created_at TEXT NOT NULL,
                last_used_at TEXT,
                aaguid TEXT,
                rp_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&self.pool)
        .await?;

        // itr#311 migration for already-deployed DBs that predate the
        // `aaguid` / `rp_id` columns. SQLite's `ADD COLUMN` is fully
        // backward-compatible (each ALTER is a single transaction) but
        // emits `duplicate column name` on a second run — swallow that
        // specific shape and surface anything else.
        try_add_column(&self.pool, "web_passkeys", "aaguid", "TEXT").await?;
        try_add_column(
            &self.pool,
            "web_passkeys",
            "rp_id",
            "TEXT NOT NULL DEFAULT ''",
        )
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
}

/// Idempotently add a column to an existing SQLite table.
///
/// SQLite's `ALTER TABLE ... ADD COLUMN` is the only structure change we
/// need for forward-compatible migrations (CREATE TABLE IF NOT EXISTS does
/// the rest). The second-run failure mode is a `Database` error whose
/// message contains `duplicate column name` — we swallow that one shape
/// and surface everything else.
///
/// Logging is at DEBUG for the already-applied case (so a noisy daemon
/// boot doesn't grow by one INFO line per migrated column) and INFO on
/// the first successful application (so the operator sees the upgrade
/// pass in the journal on the first boot after deploying).
async fn try_add_column(pool: &SqlitePool, table: &str, column: &str, col_def: &str) -> Result<()> {
    let stmt = format!("ALTER TABLE {table} ADD COLUMN {column} {col_def}");
    match sqlx::query(&stmt).execute(pool).await {
        Ok(_) => {
            info!(table, column, "added column via ALTER TABLE");
            Ok(())
        }
        Err(sqlx::Error::Database(db_err)) if is_duplicate_column_error(db_err.message()) => {
            tracing::debug!(table, column, "column already present; ALTER skipped");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Match the SQLite "column already exists" error.
///
/// **Why message-match, not error-code match:** SQLite returns
/// `SQLITE_ERROR(1)` for "duplicate column name" — but `SQLITE_ERROR(1)`
/// is also the generic catch-all (disk full, syntax error, constraint
/// failure without a more specific code, etc.). Matching the code alone
/// would silently swallow unrelated errors. The English message
/// `"duplicate column name: X"` IS the discriminator and has been
/// stable since SQLite 3.x. The message is the contract.
///
/// Localized SQLite builds (extremely rare in practice — Debian, brew,
/// MUSL, Alpine all ship the English-message build) would break this.
/// itr#320 tracks moving to a more durable discriminator if one ever
/// becomes available.
fn is_duplicate_column_error(msg: &str) -> bool {
    msg.contains("duplicate column name")
}
