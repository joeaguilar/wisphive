use anyhow::Result;
use sqlx::QueryBuilder;
use tokio::io::AsyncWriteExt;

use super::{ARCHIVE_SINK_MAX_BYTES, DecisionLogRow, StateDb, rotate_if_large};

/// Outcome of a full [`StateDb::run_retention`] pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetentionOutcome {
    /// `decision_log` rows archived to JSONL and deleted.
    pub archived: u64,
    /// `terminal_events` rows pruned for ended, aged-out sessions.
    pub terminal_events_pruned: u64,
    /// Whether a space-reclaiming VACUUM actually ran.
    pub vacuumed: bool,
}

impl RetentionOutcome {
    /// True when the pass freed nothing (used to suppress noisy logs).
    pub fn is_noop(&self) -> bool {
        self.archived == 0 && self.terminal_events_pruned == 0
    }
}

impl StateDb {
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
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64> {
        let mut total_archived = 0u64;

        // Phase 1: Archive entries older than the cutoff
        let cutoff = cutoff.to_rfc3339();
        let old_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM decision_log WHERE resolved_at < ? ORDER BY resolved_at ASC, id ASC",
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
                "SELECT id FROM decision_log ORDER BY resolved_at ASC, id ASC LIMIT ?",
            )
            .bind(excess as i64)
            .fetch_all(&self.pool)
            .await?;

            if !excess_rows.is_empty() {
                total_archived += self.archive_rows_by_ids(&excess_rows, archive_path).await?;
            }
        }

        // NOTE: space reclamation (VACUUM) is intentionally NOT done here.
        // VACUUM rewrites the entire database and can hang/OOM on a multi-GB
        // file; it is handled centrally and size-guarded by `run_retention`.
        Ok(total_archived)
    }

    /// Full retention pass run from the daemon: archive + prune `decision_log`,
    /// prune `terminal_events` for ended sessions past the age cutoff, checkpoint
    /// the WAL, and run a size-guarded VACUUM. This is the single entry point the
    /// server's startup and hourly retention should call.
    pub async fn run_retention(
        &self,
        archive_path: &std::path::Path,
        max_rows: u64,
        max_age_days: u64,
        vacuum_max_bytes: u64,
    ) -> Result<RetentionOutcome> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
        let archived = self
            .archive_and_prune(archive_path, max_rows, cutoff)
            .await?;

        let terminal_events_pruned = self.prune_terminal_events(cutoff).await?;

        // Always bound the WAL, even when nothing was reclaimed: a large WAL is
        // its own failure mode and TRUNCATE is cheap relative to VACUUM.
        if let Err(e) = self.checkpoint_wal().await {
            tracing::warn!("WAL checkpoint after retention failed: {e}");
        }

        // Only VACUUM when we actually freed rows AND the DB is small enough that
        // a full rewrite is safe. On a bloated DB we skip and warn rather than
        // risk wedging the daemon.
        let vacuumed = if archived + terminal_events_pruned > 0 {
            match self.db_size_bytes().await {
                Ok(bytes) if bytes <= vacuum_max_bytes => {
                    match sqlx::query("VACUUM").execute(&self.pool).await {
                        Ok(_) => true,
                        Err(e) => {
                            tracing::warn!("VACUUM after retention failed: {e}");
                            false
                        }
                    }
                }
                Ok(bytes) => {
                    tracing::warn!(
                        db_bytes = bytes,
                        vacuum_max_bytes,
                        "skipping VACUUM: database exceeds the safe size ceiling; \
                         run a manual VACUUM during a maintenance window"
                    );
                    false
                }
                Err(e) => {
                    tracing::warn!("could not determine DB size; skipping VACUUM: {e}");
                    false
                }
            }
        } else {
            false
        };

        Ok(RetentionOutcome {
            archived,
            terminal_events_pruned,
            vacuumed,
        })
    }

    /// Checkpoint the WAL back into the main database and truncate the WAL file.
    /// Cheap and bounded; safe to call routinely.
    pub async fn checkpoint_wal(&self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Logical database size in bytes (`page_count * page_size`). Does not need
    /// the file path and reflects pages in use, which is what gates VACUUM.
    pub async fn db_size_bytes(&self) -> Result<u64> {
        let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await?;
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await?;
        Ok((page_count.max(0) as u64).saturating_mul(page_size.max(0) as u64))
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
        // Cap the archive sink: it lives in log_dir alongside the daemon logs,
        // so a rotated segment is reaped by `logging::prune_old_files`. Without
        // this the sink grows without bound (observed at 150MB+).
        let rotation_path = archive_path.to_owned();
        tokio::task::spawn_blocking(move || {
            rotate_if_large(&rotation_path, ARCHIVE_SINK_MAX_BYTES);
        })
        .await?;

        let mut archived = 0u64;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_path)
            .await?;

        for chunk in ids.chunks(500) {
            let mut query = QueryBuilder::<sqlx::Sqlite>::new(
                "SELECT id, agent_id, agent_type, project, tool_name, tool_input, decision, \
                 requested_at, resolved_at, tool_result, tool_use_id, hook_event_name, terminal_session_id, \
                 decided_by, config_hash \
                 FROM decision_log WHERE id IN (",
            );
            let mut separated = query.separated(", ");
            for (id,) in chunk {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            let rows = query
                .build_query_as::<DecisionLogRow>()
                .fetch_all(&self.pool)
                .await?;

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
                decided_by,
                config_hash,
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
                    "decided_by": decided_by,
                    "config_hash": config_hash,
                });
                let mut line = serde_json::to_string(&entry).unwrap_or_default();
                line.push('\n');
                file.write_all(line.as_bytes()).await?;
                archived += 1;
            }
            // fsync before the DELETE commits (itr#368): `flush()` on a raw
            // File is a no-op, so without this the rows exist only in the page
            // cache while SQLite durably deletes them — a power loss in that
            // window would lose audit rows from BOTH the DB and the archive,
            // violating the "audit data is never deleted" invariant (itr#340).
            file.sync_data().await?;

            // Batch delete after archive is durably on disk
            let mut delete_query =
                QueryBuilder::<sqlx::Sqlite>::new("DELETE FROM decision_log WHERE id IN (");
            let mut separated = delete_query.separated(", ");
            for (id,) in chunk {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            delete_query.build().execute(&self.pool).await?;
        }

        Ok(archived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::{make_request, test_db};
    use wisphive_protocol::{TerminalDirection, TerminalSessionMeta, TerminalStatus};

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
            created_by: None,
            replay_acl: Vec::new(),
        }
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
            db.resolve_pending(r.id, wisphive_protocol::Decision::Approve)
                .await
                .unwrap();
        }

        // Prune to max 3 rows
        let cutoff = chrono::Utc::now() - chrono::Duration::days(365);
        let archived = db
            .archive_and_prune(&archive_path, 3, cutoff)
            .await
            .unwrap();
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

        let cutoff = chrono::Utc::now() - chrono::Duration::days(365);
        let archived = db
            .archive_and_prune(&archive_path, 100, cutoff)
            .await
            .unwrap();
        assert_eq!(archived, 0);
        assert!(!archive_path.exists());
    }

    #[tokio::test]
    async fn archive_rows_by_ids_archives_and_deletes_only_requested_ids() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("archive.jsonl");
        let requests: Vec<_> = (0..3)
            .map(|i| make_request(&format!("Tool{i}"), "cc-1", "/muse"))
            .collect();

        for request in &requests {
            db.persist_pending(request).await.unwrap();
            db.resolve_pending(request.id, wisphive_protocol::Decision::Approve)
                .await
                .unwrap();
        }

        let ids = vec![(requests[0].id.to_string(),), (requests[2].id.to_string(),)];
        assert_eq!(
            db.archive_rows_by_ids(&ids, &archive_path).await.unwrap(),
            2
        );

        let archived_ids: std::collections::BTreeSet<String> =
            std::fs::read_to_string(&archive_path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .map(|entry| entry["id"].as_str().unwrap().to_owned())
                .collect();
        let expected_ids: std::collections::BTreeSet<String> =
            ids.iter().map(|(id,)| id.clone()).collect();
        assert_eq!(archived_ids, expected_ids);

        let remaining_ids: std::collections::BTreeSet<String> = db
            .query_history(None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect();
        assert_eq!(remaining_ids, [requests[1].id.to_string()].into());
    }

    #[tokio::test]
    async fn run_retention_prunes_aged_ended_terminal_events() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();

        // An ended session whose ended_at is well past any sane retention window.
        let mut meta = make_term_meta(id);
        meta.ended_at = Some(chrono::Utc::now() - chrono::Duration::days(60));
        meta.status = TerminalStatus::Exited;
        db.create_terminal_session(&meta).await.unwrap();

        let events: Vec<_> = (0..5u64)
            .map(|seq| {
                (
                    id,
                    seq,
                    seq as i64,
                    TerminalDirection::Output,
                    b"out".to_vec(),
                )
            })
            .collect();
        db.insert_terminal_events_batch(&events).await.unwrap();
        assert_eq!(db.replay_terminal_events(id, None).await.unwrap().len(), 5);

        let archive = std::env::temp_dir().join(format!("wh-retn-{id}.jsonl"));
        let outcome = db
            .run_retention(&archive, 50_000, 30, 256 * 1024 * 1024)
            .await
            .unwrap();

        // The aged events are pruned; the session metadata row is preserved.
        assert_eq!(outcome.terminal_events_pruned, 5);
        assert_eq!(db.replay_terminal_events(id, None).await.unwrap().len(), 0);
        assert_eq!(db.list_terminal_sessions().await.unwrap().len(), 1);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rotate_if_large_rotates_only_when_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("decision_log.jsonl");

        // Under cap: untouched.
        std::fs::write(&path, b"small\n").unwrap();
        rotate_if_large(&path, 1024);
        assert!(path.exists());
        let siblings = || {
            std::fs::read_dir(tmp.path())
                .unwrap()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("decision_log.jsonl.")
                })
                .count()
        };
        assert_eq!(siblings(), 0, "no rotation under cap");

        // Over cap: original renamed to a timestamped sibling, original gone.
        std::fs::write(&path, vec![b'x'; 2048]).unwrap();
        rotate_if_large(&path, 1024);
        assert!(!path.exists(), "original should be renamed away");
        assert_eq!(siblings(), 1, "one rotated sibling expected");
    }

    #[tokio::test]
    async fn run_retention_keeps_live_session_terminal_events() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();

        // A still-running session (ended_at = None) must never be pruned.
        let meta = make_term_meta(id);
        db.create_terminal_session(&meta).await.unwrap();
        let events: Vec<_> = (0..3u64)
            .map(|seq| {
                (
                    id,
                    seq,
                    seq as i64,
                    TerminalDirection::Output,
                    b"x".to_vec(),
                )
            })
            .collect();
        db.insert_terminal_events_batch(&events).await.unwrap();

        let archive = std::env::temp_dir().join(format!("wh-retn-live-{id}.jsonl"));
        let outcome = db
            .run_retention(&archive, 50_000, 30, 256 * 1024 * 1024)
            .await
            .unwrap();

        assert_eq!(outcome.terminal_events_pruned, 0);
        assert_eq!(db.replay_terminal_events(id, None).await.unwrap().len(), 3);
        let _ = std::fs::remove_file(&archive);
    }
}
