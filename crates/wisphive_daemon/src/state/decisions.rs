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

/// Parameters for logging a hook-resolved (non-human) decision from
/// events.jsonl: an auto-approved tool call, an always-defer deferral, or a
/// fail-closed denial.
pub struct AutoApprovedEntry<'a> {
    pub agent_id: &'a str,
    pub agent_type: &'a str,
    pub project: &'a str,
    pub tool_name: &'a str,
    pub tool_input: &'a str,
    pub timestamp: &'a str,
    pub tool_use_id: Option<&'a str>,
    pub hook_event_name: Option<&'a str>,
    /// Bare decision word: "approve" (default), "ask", or "deny".
    pub decision: &'a str,
    /// The layer/rule that made the decision (itr#397), e.g. "level:all".
    pub decided_by: Option<&'a str>,
    /// Truncated SHA-256 of config.json at decision time.
    pub config_hash: Option<&'a str>,
}

impl StateDb {
    /// Persist a pending decision for crash recovery.
    pub async fn persist_pending(&self, req: &wisphive_protocol::DecisionRequest) -> Result<()> {
        // For events without tool_input (Stop, ConfigChange, etc.), store event_data instead
        // Secrets are scrubbed before anything touches disk (itr#89): this
        // row is the source for decision_log, the JSONL archive, and history
        // queries. The live in-memory queue keeps the full input for review.
        let stored_input = if req.tool_input.is_null() {
            if let Some(ref data) = req.event_data {
                wisphive_protocol::redact::redact_value(data)
            } else {
                req.tool_input.clone()
            }
        } else {
            wisphive_protocol::redact::redact_value(&req.tool_input)
        };

        // permission_suggestions (itr#300): intentionally NOT persisted. The
        // column exists from an old migration, but nothing reads it: #299
        // establishes that pending_decisions is drained (not re-served) on
        // restart, so there is no recovery read model to feed, and the live
        // in-memory queue already holds the full suggestions for review.
        // Writing them here would only risk a cleartext-secret leak — a
        // rule_content can carry a command like `curl -H "Authorization:
        // Bearer …"`, and this write bypassed the redact_value above (itr#89).
        // So the row deliberately leaves the column NULL.

        // INSERT OR IGNORE (itr#370): the id is hook-supplied, so a colliding
        // second request must never rewrite the first one's persisted row.
        // The queue rejects the duplicate; keeping the victim's row intact is
        // the defence-in-depth half.
        sqlx::query(
            "INSERT OR IGNORE INTO pending_decisions (id, agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id, hook_event_name, terminal_session_id)
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

    /// Remove a pending row WITHOUT writing to `decision_log` (itr#298).
    ///
    /// An Ask/defer is not an auditable terminal decision, so it must not land
    /// in `decision_log` — but the pending row still has to go, or it leaks:
    /// retention never reaps `pending_decisions`, and the startup drain
    /// ([`Self::drain_orphaned_pending`]) would later mis-record it as a crash
    /// orphan. Idempotent — a missing id is a no-op.
    pub async fn delete_pending(&self, id: uuid::Uuid) -> Result<()> {
        sqlx::query("DELETE FROM pending_decisions WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drain pending rows left over from a prior daemon process (itr#299).
    ///
    /// `pending_decisions` is transient in-flight bookkeeping, NOT a recovery
    /// queue. Each row's hook was blocked on a `oneshot` that died when the
    /// daemon did, and that hook already resolved ITSELF the instant the socket
    /// closed — an EOF-mid-wait is `DaemonUnreachable`, which fails **open**
    /// (approve) per ADR-0001 (see `wisphive_hook`). The daemon cannot recreate
    /// the decision or change what already happened; on restart it can only
    /// record the truthful outcome. So every orphan is logged as `Approve` /
    /// `daemon_restart:failopen` and removed.
    ///
    /// Recording a `Deny` here (as the hook-*disconnect* path does) would be an
    /// audit lie: there the tool did NOT run, but here the hook fail-open ran it.
    /// Returns the number of rows drained.
    pub async fn drain_orphaned_pending(&self) -> Result<usize> {
        let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM pending_decisions")
            .fetch_all(&self.pool)
            .await?;
        let mut drained = 0usize;
        for (id,) in ids {
            match id.parse::<uuid::Uuid>() {
                Ok(uuid) => {
                    self.resolve_pending_by(
                        uuid,
                        wisphive_protocol::Decision::Approve,
                        "daemon_restart:failopen",
                    )
                    .await?;
                    drained += 1;
                }
                Err(_) => {
                    // An unparseable id can't key a decision_log row; delete it
                    // so it can't wedge the table across every restart.
                    tracing::warn!(id, "orphaned pending row with unparseable id; deleting");
                    self.delete_pending_raw(&id).await?;
                }
            }
        }
        if drained > 0 {
            tracing::warn!(
                drained,
                "drained orphaned pending decisions as daemon_restart:failopen (itr#299)"
            );
        }
        Ok(drained)
    }

    /// Delete a pending row by its raw string id (for ids that don't parse as
    /// UUIDs — see [`Self::drain_orphaned_pending`]).
    async fn delete_pending_raw(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM pending_decisions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Number of rows currently in `pending_decisions` (test assertions).
    #[cfg(test)]
    pub(crate) async fn pending_count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pending_decisions")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    /// Remove a pending decision after resolution and log it.
    ///
    /// Shorthand for [`Self::resolve_pending_by`] with `decided_by: "human"` —
    /// the daemon path exists to put a human in the loop, so callers that
    /// don't say otherwise get the human attribution.
    pub async fn resolve_pending(
        &self,
        id: uuid::Uuid,
        decision: wisphive_protocol::Decision,
    ) -> Result<()> {
        self.resolve_pending_by(id, decision, "human").await
    }

    /// Remove a pending decision after resolution and log it, recording which
    /// actor/rule resolved it (itr#397): "human", "timeout:approve", etc.
    pub async fn resolve_pending_by(
        &self,
        id: uuid::Uuid,
        decision: wisphive_protocol::Decision,
        decided_by: &str,
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
            let result = sqlx::query(
                "INSERT OR IGNORE INTO decision_log (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id, hook_event_name, terminal_session_id, decided_by)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            .bind(decided_by)
            .execute(&self.pool)
            .await?;
            // INSERT OR IGNORE dedupes on the primary key and the unique
            // tool_use_id index. A dropped row here means this resolution
            // conflicts with one already recorded — an audit-log gap that must
            // be loud, not silent (itr#347).
            if result.rows_affected() == 0 {
                tracing::warn!(
                    %id,
                    decision = ?decision,
                    decided_by,
                    "resolution NOT recorded: decision_log already has a row for this id/tool_use_id (itr#347)"
                );
            }
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
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id, decided_by, config_hash
                         FROM decision_log WHERE agent_id = ? ORDER BY resolved_at DESC LIMIT ?",
                    )
                    .bind(aid)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_as(
                        "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id, decided_by, config_hash
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
        // Tool responses carry file contents / command output that routinely
        // include credentials — scrub before persisting (itr#89).
        let result_json =
            serde_json::to_string(&wisphive_protocol::redact::redact_value(tool_result))?;

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
        if let Some(since) = search.since {
            conditions.push("resolved_at >= ?".to_string());
            binds.push(since.to_rfc3339());
        }
        if let Some(ref project) = search.project {
            conditions.push("project = ?".to_string());
            binds.push(project.clone());
        }
        if let Some(ref rule) = search.decided_by {
            conditions.push("decided_by LIKE '%' || ? || '%'".to_string());
            binds.push(rule.clone());
        }

        let where_clause = if conditions.is_empty() {
            "1=1".to_string()
        } else {
            conditions.join(" AND ")
        };

        let sql = format!(
            "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id, decided_by, config_hash
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
        // `decision` is stored as a JSON-encoded string to match the rows
        // resolve_pending writes. auto_approved=1 only for actual approvals —
        // deferrals/denials are audit rows, not approvals.
        let decision = match entry.decision {
            "" => "approve",
            other => other,
        };
        sqlx::query(
            "INSERT OR IGNORE INTO decision_log
             (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, auto_approved, tool_use_id, hook_event_name, decided_by, config_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(entry.agent_id)
        .bind(entry.agent_type)
        .bind(entry.project)
        .bind(entry.tool_name)
        .bind(entry.tool_input)
        .bind(format!("\"{decision}\""))
        .bind(entry.timestamp)
        .bind(entry.timestamp)
        .bind(i64::from(decision == "approve"))
        .bind(entry.tool_use_id)
        .bind(entry.hook_event_name.unwrap_or("PreToolUse"))
        .bind(entry.decided_by)
        .bind(entry.config_hash)
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
                decided_by,
                config_hash,
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
                    decided_by,
                    config_hash,
                })
            },
        )
        .collect()
}

#[cfg(test)]
#[path = "decisions_tests.rs"]
mod tests;
