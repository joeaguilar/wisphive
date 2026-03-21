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

        // Enable WAL mode for crash safety
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Persist a pending decision for crash recovery.
    pub async fn persist_pending(&self, req: &wisphive_protocol::DecisionRequest) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO pending_decisions (id, agent_id, agent_type, project, tool_name, tool_input, timestamp)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(req.id.to_string())
        .bind(&req.agent_id)
        .bind(serde_json::to_string(&req.agent_type)?)
        .bind(req.project.to_string_lossy().to_string())
        .bind(&req.tool_name)
        .bind(serde_json::to_string(&req.tool_input)?)
        .bind(req.timestamp.to_rfc3339())
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
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT agent_id, agent_type, project, tool_name, tool_input, timestamp
             FROM pending_decisions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some((agent_id, agent_type, project, tool_name, tool_input, requested_at)) = row {
            sqlx::query(
                "INSERT INTO decision_log (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        let rows: Vec<(String, String, String, String, String, String, String, String, String)> =
            match agent_id {
                Some(aid) => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at
                         FROM decision_log WHERE agent_id = ? ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(aid)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at
                         FROM decision_log ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
            };

        let entries = rows
            .into_iter()
            .filter_map(|(id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at)| {
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
                })
            })
            .collect();

        Ok(entries)
    }

    /// Get the underlying pool for direct queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
