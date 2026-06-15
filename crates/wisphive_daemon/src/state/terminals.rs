use anyhow::Result;
use wisphive_protocol::{TerminalDirection, TerminalSessionMeta, TerminalStatus};

use super::StateDb;

impl StateDb {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_db;

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
}
