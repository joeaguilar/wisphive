use anyhow::Result;

use super::{DecisionLogRow, StateDb};

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

impl StateDb {
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
}

/// Convert raw SQL rows to HistoryEntry structs.
pub(super) fn rows_to_entries(rows: Vec<DecisionLogRow>) -> Vec<wisphive_protocol::HistoryEntry> {
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
#[path = "decisions_tests.rs"]
mod tests;
