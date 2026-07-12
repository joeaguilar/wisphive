use anyhow::Result;
use sqlx::QueryBuilder;

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

/// Decision-log row shape for compact recent audit snapshots.
type AuditDecisionRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    // tool_use_id — stable key a `deferred_resolved` correlates against (itr#462)
    Option<String>,
    // tool_result — non-NULL on a deferral means it was answered (itr#461)
    Option<String>,
);

/// Outcome of [`StateDb::attach_tool_result`]: the matched decision_log row's id
/// and whether it was a DEFERRED native prompt (`decision = "ask"`), which the
/// server uses to decide whether to broadcast a `DeferredResolved` (itr#461).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachedResult {
    pub id: uuid::Uuid,
    pub was_deferred: bool,
}

impl AttachedResult {
    /// Build from the raw `id` + `decision` columns. Returns None only if the id
    /// fails to parse (unreachable for daemon-written rows, kept for total safety).
    fn from_row(id_str: &str, decision: &str) -> Option<Self> {
        Some(Self {
            id: id_str.parse().ok()?,
            // `decision` is stored JSON-encoded, so a deferral is the literal `"ask"`.
            was_deferred: decision == "\"ask\"",
        })
    }
}

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

/// Start another parameterized predicate in a dynamically filtered history
/// query. Predicate text is static SQL; caller-provided values are added by
/// the caller through [`QueryBuilder::push_bind`].
fn push_history_condition(
    query: &mut QueryBuilder<'_, sqlx::Sqlite>,
    has_where: &mut bool,
    predicate: &str,
) {
    query.push(if *has_where { " AND " } else { " WHERE " });
    *has_where = true;
    query.push(predicate);
}

/// Quote arbitrary text as one literal FTS5 phrase.
///
/// Binding prevents SQL injection, while quoting also prevents FTS operators
/// such as `OR`, `*`, and column filters from changing search semantics.
fn fts_trigram_phrase(text: &str) -> String {
    let mut phrase = String::with_capacity(text.len() + 2);
    phrase.push('"');
    for character in text.chars() {
        if character == '"' {
            phrase.push('"');
        }
        phrase.push(character);
    }
    phrase.push('"');
    phrase
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

    /// Confirm that a pending id belongs to the daemon-created managed-spawn
    /// namespace. Used after INSERT OR IGNORE so a UUID collision can never
    /// turn an unrelated hook row into an executable spawn approval.
    pub async fn pending_is_managed_spawn(&self, id: uuid::Uuid) -> Result<bool> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pending_decisions \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?;
        Ok(count == 1)
    }

    /// Delete only a daemon-provenance SpawnAgent pending row. Returns false
    /// for a missing/colliding hook row so the caller can fail closed.
    pub async fn delete_managed_spawn_pending(&self, id: uuid::Uuid) -> Result<bool> {
        let deleted = sqlx::query(
            "DELETE FROM pending_decisions \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected() == 1)
    }

    /// Drain pending rows left over from a prior daemon process (itr#299).
    ///
    /// `pending_decisions` is transient in-flight bookkeeping, NOT a recovery
    /// queue. Hook rows were blocked on a `oneshot` that died when the daemon
    /// did, and those hooks already resolved themselves fail-open per ADR-0001.
    /// Synthetic `SpawnAgent` rows are different: no child is launched until
    /// their in-daemon approval receiver fires, so a daemon crash means the
    /// action definitely did **not** execute. Those rows are drained as Deny /
    /// `daemon_restart:failclosed_spawn`; hook rows retain the truthful Approve /
    /// `daemon_restart:failopen` outcome.
    ///
    /// Recording a `Deny` for a hook row would be an audit lie because that hook
    /// already ran its tool; the SpawnAgent special case is denied precisely
    /// because no child ran.
    /// Returns the number of rows drained.
    pub async fn drain_orphaned_pending(&self) -> Result<usize> {
        let ids: Vec<(String, String, String)> =
            sqlx::query_as("SELECT id, agent_id, tool_name FROM pending_decisions")
                .fetch_all(&self.pool)
                .await?;
        let mut drained = 0usize;
        let mut failclosed_spawns = 0usize;
        for (id, agent_id, tool_name) in ids {
            match id.parse::<uuid::Uuid>() {
                Ok(uuid) => {
                    if agent_id == "wisphive-daemon:spawn" && tool_name == "SpawnAgent" {
                        self.resolve_pending_by(
                            uuid,
                            wisphive_protocol::Decision::Deny,
                            "daemon_restart:failclosed_spawn",
                        )
                        .await?;
                        failclosed_spawns += 1;
                    } else {
                        self.resolve_pending_by(
                            uuid,
                            wisphive_protocol::Decision::Approve,
                            "daemon_restart:failopen",
                        )
                        .await?;
                    }
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
                failclosed_spawns,
                "drained orphaned pending decisions after daemon restart (itr#299, itr#94)"
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

    /// Persist an approved, fully reviewed SpawnAgent request atomically before
    /// its queue receiver is released. The reviewed request may differ from the
    /// originally queued one, so agent type, project, and redacted tool input
    /// are updated together with the audit move. Returns false if no pending row
    /// existed or the audit insert conflicted; callers must fail closed then.
    pub async fn resolve_spawn_pending_by(
        &self,
        id: uuid::Uuid,
        reviewed: &wisphive_protocol::SpawnAgentRequest,
        decided_by: &str,
    ) -> Result<bool> {
        let reviewed_input =
            wisphive_protocol::redact::redact_value(&serde_json::to_value(reviewed)?);
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE pending_decisions SET agent_type = ?, project = ?, tool_input = ? \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(serde_json::to_string(&reviewed.agent_type)?)
        .bind(reviewed.project.to_string_lossy().to_string())
        .bind(serde_json::to_string(&reviewed_input)?)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }

        let row = sqlx::query_as::<_, PendingRowWithTerm>(
            "SELECT agent_id, agent_type, project, tool_name, tool_input, timestamp, tool_use_id, hook_event_name, terminal_session_id \
             FROM pending_decisions \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let (
            agent_id,
            agent_type,
            project,
            tool_name,
            tool_input,
            requested_at,
            tool_use_id,
            hook_event_name,
            terminal_session_id,
        ) = row;

        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO decision_log (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id, hook_event_name, terminal_session_id, decided_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(agent_id)
        .bind(agent_type)
        .bind(project)
        .bind(tool_name)
        .bind(tool_input)
        .bind(serde_json::to_string(
            &wisphive_protocol::Decision::Approve,
        )?)
        .bind(requested_at)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(tool_use_id)
        .bind(hook_event_name)
        .bind(terminal_session_id)
        .bind(decided_by)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM pending_decisions WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        if inserted.rows_affected() == 0 {
            tracing::warn!(
                %id,
                decided_by,
                "SpawnAgent approval NOT recorded due to decision-log conflict; refusing launch"
            );
            return Ok(false);
        }
        Ok(true)
    }

    /// Reconcile a managed spawn to a durable fail-closed outcome.
    ///
    /// A timeout can cancel the future that was moving a pending row into the
    /// audit log without proving whether SQLite committed it. This transaction
    /// deliberately waits behind that write, then overwrites a committed
    /// approval or moves the still-pending row as a Deny. The queue receiver is
    /// released with Deny before callers await this reconciliation, so no
    /// process can launch while the database settles.
    pub async fn force_failclosed_spawn_resolution(
        &self,
        id: uuid::Uuid,
        decided_by: &str,
        fallback: &wisphive_protocol::DecisionRequest,
    ) -> Result<bool> {
        if fallback.id != id
            || fallback.agent_id != "wisphive-daemon:spawn"
            || fallback.tool_name != "SpawnAgent"
        {
            anyhow::bail!("fail-closed SpawnAgent fallback lacks daemon provenance");
        }
        let id = id.to_string();
        let denied = serde_json::to_string(&wisphive_protocol::Decision::Deny)?;
        let resolved_at = chrono::Utc::now().to_rfc3339();
        let fallback_input = wisphive_protocol::redact::redact_value(&fallback.tool_input);
        let mut tx = self.pool.begin().await?;

        // If an ambiguously timed-out approval actually committed, make its
        // durable outcome fail closed before considering the reconciliation
        // complete.
        sqlx::query(
            "UPDATE decision_log SET decision = ?, resolved_at = ?, decided_by = ? \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(&denied)
        .bind(&resolved_at)
        .bind(decided_by)
        .bind(&id)
        .execute(&mut *tx)
        .await?;

        // Otherwise move the original synthetic row into the audit log. The
        // internal agent id is reserved at the socket boundary, so this pair
        // is daemon provenance rather than a hook-supplied label.
        sqlx::query(
            "INSERT OR IGNORE INTO decision_log \
             (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id, hook_event_name, terminal_session_id, decided_by) \
             SELECT id, agent_id, agent_type, project, tool_name, tool_input, ?, timestamp, ?, tool_use_id, hook_event_name, terminal_session_id, ? \
             FROM pending_decisions \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(&denied)
        .bind(&resolved_at)
        .bind(decided_by)
        .bind(&id)
        .execute(&mut *tx)
        .await?;

        // A timed-out Ask cleanup may have committed its DELETE just before
        // cancellation, leaving neither pending nor audit state. Reconstruct a
        // truthful non-execution Deny from the daemon-owned in-memory request.
        sqlx::query(
            "INSERT OR IGNORE INTO decision_log \
             (id, agent_id, agent_type, project, tool_name, tool_input, decision, requested_at, resolved_at, tool_use_id, hook_event_name, terminal_session_id, decided_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&fallback.agent_id)
        .bind(serde_json::to_string(&fallback.agent_type)?)
        .bind(fallback.project.to_string_lossy().to_string())
        .bind(&fallback.tool_name)
        .bind(serde_json::to_string(&fallback_input)?)
        .bind(&denied)
        .bind(fallback.timestamp.to_rfc3339())
        .bind(&resolved_at)
        .bind(&fallback.tool_use_id)
        .bind(fallback.hook_event_name.to_string())
        .bind(fallback.terminal_session_id.map(|id| id.to_string()))
        .bind(decided_by)
        .execute(&mut *tx)
        .await?;

        // Repeat the update so INSERT conflicts cannot preserve an approval.
        let reconciled = sqlx::query(
            "UPDATE decision_log SET decision = ?, resolved_at = ?, decided_by = ? \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(&denied)
        .bind(&resolved_at)
        .bind(decided_by)
        .bind(&id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        sqlx::query(
            "DELETE FROM pending_decisions \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' AND tool_name = 'SpawnAgent'",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(reconciled)
    }

    /// Attach the execution failure that happened after a reviewed approval.
    /// The approval remains truthful, while `decided_by` and `tool_result`
    /// make it durable that the approved action did not start successfully.
    pub async fn record_spawn_action_failure(&self, id: uuid::Uuid, error: &str) -> Result<bool> {
        let result = wisphive_protocol::redact::redact_value(&serde_json::json!({
            "spawn_status": "action_failed",
            "error": error,
        }));
        let updated = sqlx::query(
            "UPDATE decision_log \
             SET tool_result = ?, \
                 decided_by = CASE \
                   WHEN decided_by LIKE '%:spawn_action_failed' THEN decided_by \
                   ELSE COALESCE(decided_by, 'unknown') || ':spawn_action_failed' \
                 END \
             WHERE id = ? AND agent_id = 'wisphive-daemon:spawn' \
               AND tool_name = 'SpawnAgent' AND decision = ?",
        )
        .bind(serde_json::to_string(&result)?)
        .bind(id.to_string())
        .bind(serde_json::to_string(
            &wisphive_protocol::Decision::Approve,
        )?)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
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
    ///
    /// Returns the matched row's id plus whether it was a DEFERRED native prompt
    /// (`decision = "ask"`). A deferred match means the human just answered a
    /// prompt that the inbox is still showing as "waiting in your terminal", so
    /// the caller broadcasts `DeferredResolved` to clear it (itr#461).
    pub async fn attach_tool_result(
        &self,
        agent_id: &str,
        tool_name: &str,
        tool_result: &serde_json::Value,
        tool_use_id: Option<&str>,
    ) -> Result<Option<AttachedResult>> {
        // Tool responses carry file contents / command output that routinely
        // include credentials — scrub before persisting (itr#89).
        let result_json =
            serde_json::to_string(&wisphive_protocol::redact::redact_value(tool_result))?;

        // Try exact match by tool_use_id first
        if let Some(tui) = tool_use_id {
            let row: Option<(String, String)> = sqlx::query_as(
                "SELECT id, decision FROM decision_log
                 WHERE tool_use_id = ? AND tool_result IS NULL
                 LIMIT 1",
            )
            .bind(tui)
            .fetch_optional(&self.pool)
            .await?;

            if let Some((id_str, decision)) = row {
                sqlx::query("UPDATE decision_log SET tool_result = ? WHERE id = ?")
                    .bind(&result_json)
                    .bind(&id_str)
                    .execute(&self.pool)
                    .await?;
                return Ok(AttachedResult::from_row(&id_str, &decision));
            }
        }

        // Fallback: fuzzy match by agent_id + tool_name + recency
        let cutoff = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT id, decision FROM decision_log
             WHERE agent_id = ? AND tool_name = ? AND tool_result IS NULL
             AND resolved_at > ?
             ORDER BY resolved_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .bind(tool_name)
        .bind(&cutoff)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id_str, decision)) = row {
            sqlx::query("UPDATE decision_log SET tool_result = ? WHERE id = ?")
                .bind(&result_json)
                .bind(&id_str)
                .execute(&self.pool)
                .await?;
            Ok(AttachedResult::from_row(&id_str, &decision))
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

        // Empty, whitespace-only, and punctuation-only input has no searchable
        // term. It therefore omits the text predicate while preserving any
        // exact filters below, matching the old empty-LIKE behaviour.
        let text_filter = search
            .query
            .as_deref()
            .filter(|text| text.chars().any(char::is_alphanumeric));
        let use_fts =
            text_filter.is_some_and(|text| text.chars().count() >= 3 && !text.contains('\0'));

        let mut query = QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT d.id, d.agent_id, d.agent_type, d.project, d.tool_name, d.tool_input, \
             d.decision, d.requested_at, d.resolved_at, d.tool_result, d.tool_use_id, \
             d.hook_event_name, d.terminal_session_id, d.decided_by, d.config_hash \
             FROM decision_log AS d",
        );
        if use_fts {
            query.push(" JOIN decision_log_fts ON decision_log_fts.rowid = d.rowid");
        }

        let mut has_where = false;
        if let Some(text) = text_filter {
            if use_fts {
                push_history_condition(&mut query, &mut has_where, "decision_log_fts MATCH ");
                query.push_bind(fts_trigram_phrase(text));
            } else {
                // FTS5 trigram queries shorter than three Unicode characters
                // cannot use the index, and its query parser rejects embedded
                // NUL. Preserve graceful type-ahead with bound LIKE for these
                // cases.
                push_history_condition(&mut query, &mut has_where, "(d.tool_input LIKE '%' || ");
                query.push_bind(text);
                query.push(" || '%' OR d.tool_result LIKE '%' || ");
                query.push_bind(text);
                query.push(" || '%' OR d.tool_name LIKE '%' || ");
                query.push_bind(text);
                query.push(" || '%')");
            }
        }
        if let Some(ref tool) = search.tool_name {
            push_history_condition(&mut query, &mut has_where, "d.tool_name = ");
            query.push_bind(tool);
        }
        if let Some(ref aid) = search.agent_id {
            push_history_condition(&mut query, &mut has_where, "d.agent_id = ");
            query.push_bind(aid);
        }
        if let Some(since) = search.since {
            push_history_condition(&mut query, &mut has_where, "d.resolved_at >= ");
            query.push_bind(since.to_rfc3339());
        }
        if let Some(ref project) = search.project {
            push_history_condition(&mut query, &mut has_where, "d.project = ");
            query.push_bind(project);
        }
        if let Some(ref rule) = search.decided_by {
            push_history_condition(&mut query, &mut has_where, "d.decided_by LIKE '%' || ");
            query.push_bind(rule);
            query.push(" || '%'");
        }

        query.push(" ORDER BY d.resolved_at DESC LIMIT ");
        query.push_bind(limit);

        let rows = query
            .build_query_as::<DecisionLogRow>()
            .fetch_all(&self.pool)
            .await?;
        Ok(rows_to_entries(rows))
    }

    /// Recent non-human audit decisions for live clients joining the stream.
    pub async fn recent_audit_decisions(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: u32,
    ) -> Result<Vec<wisphive_protocol::AuditDecision>> {
        let rows: Vec<AuditDecisionRow> = sqlx::query_as(
            "SELECT agent_id, project, tool_name, decision, resolved_at, requested_at,
                    terminal_session_id, decided_by, auto_approved, tool_use_id, tool_result
             FROM decision_log
             WHERE resolved_at >= ?
               AND (
                    auto_approved = 1
                    OR decision = '\"ask\"'
                    OR (
                        decision = '\"deny\"'
                        AND decided_by IS NOT NULL
                        AND decided_by NOT LIKE 'human:%'
                        AND decided_by != 'human'
                    )
               )
             ORDER BY resolved_at DESC
             LIMIT ?",
        )
        .bind(since.to_rfc3339())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(
                |(
                    agent_id,
                    project,
                    tool_name,
                    decision,
                    resolved_at,
                    requested_at,
                    terminal_session_id,
                    decided_by,
                    auto_approved,
                    tool_use_id,
                    tool_result,
                )| {
                    let kind =
                        audit_kind_from_row(auto_approved, &decision, decided_by.as_deref())?;
                    let ts = chrono::DateTime::parse_from_rfc3339(&resolved_at)
                        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&requested_at))
                        .ok()?
                        .with_timezone(&chrono::Utc);
                    // For a DEFERRED row, a non-NULL tool_result means the native prompt
                    // was answered (the daemon stamped it via attach_tool_result), so a
                    // client reconnecting mid-hour renders it resolved rather than waiting
                    // (itr#461). Only meaningful for deferrals; None otherwise.
                    let resolved = match kind {
                        wisphive_protocol::AuditDecisionKind::Deferred => {
                            Some(tool_result.is_some())
                        }
                        _ => None,
                    };
                    Some(wisphive_protocol::AuditDecision {
                        kind,
                        decided_by,
                        project: std::path::PathBuf::from(project),
                        agent_id,
                        terminal_session_id: terminal_session_id
                            .as_deref()
                            .and_then(|s| uuid::Uuid::parse_str(s).ok()),
                        tool_name,
                        ts,
                        tool_use_id,
                        resolved,
                        // Snapshot seed is served from SQLite; the redacted
                        // deferred tool_input rides the live ingest wire only.
                        tool_input: None,
                    })
                },
            )
            .collect())
    }

    /// Get the underlying pool for direct queries.
    /// Insert an auto-approved tool call directly into decision_log.
    /// Called by the event ingest task when processing events.jsonl.
    pub async fn log_auto_approved(&self, entry: &AutoApprovedEntry<'_>) -> Result<bool> {
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
        let result = sqlx::query(
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
        Ok(result.rows_affected() > 0)
    }
}

fn audit_kind_from_row(
    auto_approved: i64,
    decision: &str,
    decided_by: Option<&str>,
) -> Option<wisphive_protocol::AuditDecisionKind> {
    if auto_approved == 1 {
        return Some(wisphive_protocol::AuditDecisionKind::AutoApproved);
    }
    match decision {
        "\"ask\"" => Some(wisphive_protocol::AuditDecisionKind::Deferred),
        "\"deny\"" if decided_by.is_some_and(|by| by != "human" && !by.starts_with("human:")) => {
            Some(wisphive_protocol::AuditDecisionKind::Denied)
        }
        _ => None,
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
