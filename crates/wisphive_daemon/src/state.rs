use anyhow::Result;
use sqlx::SqlitePool;
use tracing::info;

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

        // Index for exact tool_use_id correlation
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_decision_log_tool_use_id
             ON decision_log(tool_use_id)",
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
        sqlx::query(
            "INSERT OR REPLACE INTO pending_decisions (id, agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(req.id.to_string())
        .bind(&req.agent_id)
        .bind(serde_json::to_string(&req.agent_type)?)
        .bind(req.project.to_string_lossy().to_string())
        .bind(&req.tool_name)
        .bind(serde_json::to_string(&req.tool_input)?)
        .bind(req.timestamp.to_rfc3339())
        .bind(&req.tool_use_id)
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
        let row = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>)>(
            "SELECT agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id
             FROM pending_decisions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some((agent_id, agent_type, project, tool_name, tool_input, requested_at, tool_use_id)) = row {
            sqlx::query(
                "INSERT INTO decision_log (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        let rows: Vec<(String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)> =
            match agent_id {
                Some(aid) => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id
                         FROM decision_log WHERE agent_id = ? ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(aid)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id
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
            "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id
             FROM decision_log WHERE {} ORDER BY resolved_at DESC LIMIT ?",
            where_clause
        );

        let mut query = sqlx::query_as::<_, (String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)>(&sql);
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
    pub async fn log_auto_approved(
        &self,
        agent_id: &str,
        agent_type: &str,
        project: &str,
        tool_name: &str,
        tool_input: &str,
        timestamp: &str,
        tool_use_id: Option<&str>,
    ) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT OR IGNORE INTO decision_log
             (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, auto_approved, tool_use_id)
             VALUES (?, ?, ?, ?, ?, ?, '\"approve\"', ?, ?, 1, ?)",
        )
        .bind(&id)
        .bind(agent_id)
        .bind(agent_type)
        .bind(project)
        .bind(tool_name)
        .bind(tool_input)
        .bind(timestamp)
        .bind(timestamp)
        .bind(tool_use_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Query distinct sessions from decision_log with aggregated stats.
    pub async fn query_sessions(&self) -> Result<Vec<wisphive_protocol::SessionSummary>> {
        let rows: Vec<(String, String, String, String, String, i64, i64, i64)> = sqlx::query_as(
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
}

/// Convert raw SQL rows to HistoryEntry structs.
fn rows_to_entries(
    rows: Vec<(String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)>,
) -> Vec<wisphive_protocol::HistoryEntry> {
    rows.into_iter()
        .filter_map(|(id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id)| {
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
            })
        })
        .collect()
}
