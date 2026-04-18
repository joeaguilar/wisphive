use anyhow::Result;
use sqlx::SqlitePool;
use tracing::info;
use wisphive_protocol::{TerminalDirection, TerminalSessionMeta, TerminalStatus};

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

/// Manages the SQLite state database for crash recovery and audit.
pub struct StateDb {
    pool: SqlitePool,
}

impl StateDb {
    /// Open (or create) the database at the given path.
    pub async fn open(path: &str) -> Result<Self> {
        let url = format!("sqlite:{}?mode=rwc", path);
        let pool = SqlitePool::connect(&url).await?;

        let db = Self { pool };
        db.migrate().await?;
        // Any terminal session still marked running at startup belongs to a
        // prior daemon instance whose PTY is gone. Mark orphaned so replay
        // still works but clients know the live stream is unreachable.
        db.mark_running_terminals_orphaned().await?;
        info!("state database ready at {}", path);
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
        sqlx::query("ALTER TABLE pending_decisions ADD COLUMN hook_event_name TEXT DEFAULT 'PreToolUse'")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE decision_log ADD COLUMN hook_event_name TEXT DEFAULT 'PreToolUse'")
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
        sqlx::query("ALTER TABLE terminal_sessions ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0")
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

        if let Some((agent_id, agent_type, project, tool_name, tool_input, requested_at, tool_use_id, hook_event_name, terminal_session_id)) = row {
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
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
                0, 0, 0, 0, 0, 0, 0, 0,
            ]).to_string()
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
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(max_age_days as i64)).to_rfc3339();
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
            let excess_rows: Vec<(String,)> = sqlx::query_as(
                "SELECT id FROM decision_log ORDER BY resolved_at ASC LIMIT ?",
            )
            .bind(excess as i64)
            .fetch_all(&self.pool)
            .await?;

            if !excess_rows.is_empty() {
                total_archived += self.archive_rows_by_ids(&excess_rows, archive_path).await?;
            }
        }

        // Reclaim disk space if we archived anything
        if total_archived > 0
            && let Err(e) = sqlx::query("VACUUM").execute(&self.pool).await {
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
            for (id, agent_id, _agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id) in &rows {
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
                |(agent_id, agent_type, project, first_seen, last_seen, total, approved, denied)| {
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
    pub async fn set_terminal_group(
        &self,
        id: uuid::Uuid,
        group: Option<&str>,
    ) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET group_name = ? WHERE id = ?")
            .bind(group)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update a session's manual sort order.
    pub async fn set_terminal_sort_order(
        &self,
        id: uuid::Uuid,
        sort_order: i64,
    ) -> Result<()> {
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
            let Ok(id) = uuid::Uuid::parse_str(&id) else { continue };
            let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
            let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&started_at) else { continue };
            let ended_at = ended_at
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&chrono::Utc));
            let Ok(status) = status.parse::<TerminalStatus>() else { continue };
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
    pub async fn get_terminal_session(&self, id: uuid::Uuid) -> Result<Option<TerminalSessionMeta>> {
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
            let Ok(direction) = dir.parse::<TerminalDirection>() else { continue };
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
    pub async fn prune_terminal_events(&self, cutoff: chrono::DateTime<chrono::Utc>) -> Result<u64> {
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
fn rows_to_entries(
    rows: Vec<DecisionLogRow>,
) -> Vec<wisphive_protocol::HistoryEntry> {
    rows.into_iter()
        .filter_map(|(id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id)| {
            Some(wisphive_protocol::HistoryEntry {
                id: id.parse().ok()?,
                agent_id,
                agent_type: serde_json::from_str(&agent_type).ok()?,
                project: std::path::PathBuf::from(project),
                tool_name,
                tool_input: serde_json::from_str(&tool_input).unwrap_or(serde_json::Value::Null),
                decision: serde_json::from_str(&decision).ok()?,
                requested_at: chrono::DateTime::parse_from_rfc3339(&requested_at).ok()?.with_timezone(&chrono::Utc),
                resolved_at: chrono::DateTime::parse_from_rfc3339(&resolved_at).ok()?.with_timezone(&chrono::Utc),
                tool_result: tool_result.and_then(|s| serde_json::from_str(&s).ok()),
                tool_use_id,
                hook_event_name,
                terminal_session_id: terminal_session_id.as_deref().and_then(|s| uuid::Uuid::parse_str(s).ok()),
            })
        })
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

    fn make_request_with_tool_use_id(tool: &str, agent_id: &str, tool_use_id: &str) -> DecisionRequest {
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
        db.resolve_pending(fake_id, Decision::Approve).await.unwrap();
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
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Read", "{}", "2024-01-01T00:00:00Z", Some("tui-1"), Some("PreToolUse")).await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "Read");
        assert_eq!(history[0].decision, Decision::Approve);
    }

    #[tokio::test]
    async fn log_auto_approved_dedup_with_tool_use_id() {
        let db = test_db().await;

        // Insert same event twice with same tool_use_id
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Read", "{}", "2024-01-01T00:00:00Z", Some("tui-1"), Some("PreToolUse")).await;
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Read", "{}", "2024-01-01T00:00:00Z", Some("tui-1"), Some("PreToolUse")).await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1, "duplicate with same tool_use_id should be ignored");
    }

    /// Fixed #58: Events without tool_use_id are now deduplicated via
    /// deterministic content-hashed IDs in log_auto_approved().
    #[tokio::test]
    async fn log_auto_approved_dedup_without_tool_use_id() {
        let db = test_db().await;

        // Insert same event twice with NO tool_use_id
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Read", "{}", "2024-01-01T00:00:00Z", None, Some("PreToolUse")).await;
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Read", "{}", "2024-01-01T00:00:00Z", None, Some("PreToolUse")).await;

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1, "deterministic IDs should deduplicate events without tool_use_id");
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
        let matched = db.attach_tool_result("cc-1", "Bash", &result, Some("tui-456")).await.unwrap();
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
        let matched = db.attach_tool_result("cc-1", "Bash", &result, None).await.unwrap();
        assert!(matched.is_some());
    }

    #[tokio::test]
    async fn attach_tool_result_no_match() {
        let db = test_db().await;
        let result = serde_json::json!({"output": "orphan"});
        let matched = db.attach_tool_result("cc-99", "Bash", &result, None).await.unwrap();
        assert!(matched.is_none());
    }

    #[tokio::test]
    async fn attach_tool_result_does_not_overwrite() {
        let db = test_db().await;
        let req = make_request_with_tool_use_id("Bash", "cc-1", "tui-789");
        db.persist_pending(&req).await.unwrap();
        db.resolve_pending(req.id, Decision::Approve).await.unwrap();

        let r1 = serde_json::json!({"output": "first"});
        db.attach_tool_result("cc-1", "Bash", &r1, Some("tui-789")).await.unwrap();

        // Second attach to same tool_use_id should find no match (already has result)
        let r2 = serde_json::json!({"output": "second"});
        let matched = db.attach_tool_result("cc-1", "Bash", &r2, Some("tui-789")).await.unwrap();
        assert!(matched.is_none(), "should not overwrite existing tool_result");
    }

    // ════════════════════════════════════════════════════════════
    // search_history
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn search_history_by_query() {
        let db = test_db().await;

        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Bash", "{\"command\":\"cargo build\"}", "2024-01-01T00:00:00Z", Some("a"), None).await;
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Edit", "{\"file\":\"main.rs\"}", "2024-01-01T00:01:00Z", Some("b"), None).await;

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

        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Bash", "{}", "2024-01-01T00:00:00Z", Some("a"), None).await;
        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Edit", "{}", "2024-01-01T00:01:00Z", Some("b"), None).await;

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

        log_auto(&db, "cc-1", "\"claude_code\"", "/muse", "Bash", "{}", "2024-01-01T00:00:00Z", Some("a"), None).await;
        log_auto(&db, "cc-2", "\"claude_code\"", "/rpg", "Bash", "{}", "2024-01-01T00:01:00Z", Some("b"), None).await;

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

        let muse = projects.iter().find(|p| p.project == std::path::PathBuf::from("/muse")).unwrap();
        assert_eq!(muse.total_calls, 2);
        assert_eq!(muse.agent_count, 2);
        assert_eq!(muse.approved, 1);
        assert_eq!(muse.denied, 1);

        let rpg = projects.iter().find(|p| p.project == std::path::PathBuf::from("/rpg")).unwrap();
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
        db.create_terminal_session(&make_term_meta(id)).await.unwrap();

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
        db.create_terminal_session(&make_term_meta(id)).await.unwrap();
        db.end_terminal_session(id, Some(0), TerminalStatus::Exited).await.unwrap();

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Exited);
        assert_eq!(got.exit_code, Some(0));
        assert!(got.ended_at.is_some());
    }

    #[tokio::test]
    async fn terminal_events_batch_and_replay_preserve_order_and_bytes() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id)).await.unwrap();

        let rows = vec![
            (id, 1u64, 100i64, TerminalDirection::Output, b"hello\n".to_vec()),
            (id, 2, 200, TerminalDirection::Input, b"yes\r".to_vec()),
            (id, 3, 300, TerminalDirection::Output, vec![0x1b, b'[', b'3', b'1', b'm']),
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
            db.create_terminal_session(&make_term_meta(*id)).await.unwrap();
        }

        // Assign the first two to a group, reorder them.
        db.set_terminal_group(ids[0], Some("frontend")).await.unwrap();
        db.set_terminal_group(ids[1], Some("frontend")).await.unwrap();
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
        db.create_terminal_session(&make_term_meta(id)).await.unwrap();
        // Directly invoke the sweeper (also runs inside StateDb::open).
        db.mark_running_terminals_orphaned().await.unwrap();
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Orphaned);
        assert!(got.ended_at.is_some());
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
