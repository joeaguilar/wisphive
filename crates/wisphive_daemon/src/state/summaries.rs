use anyhow::Result;

use super::StateDb;

/// Row shape for session aggregate queries (8 columns).
type SessionRow = (String, String, String, String, String, i64, i64, i64);

impl StateDb {
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

    /// Recent approved file-touching tool calls for one project, newest first
    /// (itr#401 working-tree attribution). Returns
    /// `(agent_id, tool_name, tool_input_json)` rows for
    /// Edit/Write/MultiEdit/NotebookEdit/Bash — the tools whose inputs carry
    /// the file paths the working-tree strip attributes changes against.
    /// `tool_input` is already secret-redacted upstream (itr#89).
    pub async fn recent_file_touches(
        &self,
        project: &str,
        limit: u32,
    ) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT agent_id, tool_name, tool_input
             FROM decision_log
             WHERE project = ?1
               AND decision = '\"approve\"'
               AND tool_name IN ('Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'Bash')
             ORDER BY resolved_at DESC
             LIMIT ?2",
        )
        .bind(project)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Recent approved artifact-candidate tool calls across ALL projects,
    /// newest first (itr#402 burn meter, spec §5.4). Returns
    /// `(agent_id, project, tool_name, tool_input_json, resolved_at)` rows for
    /// Edit/Write/MultiEdit/NotebookEdit/Bash — the same candidate set as
    /// [`Self::recent_file_touches`] (itr#401); the web client classifies them
    /// into artifact signals (file writes, `git commit`) so the honest-proxy
    /// math stays unit-testable in one place. `since` is an RFC 3339 timestamp
    /// compared lexicographically, matching how `resolved_at` is stored (the
    /// existing MIN/MAX aggregates rely on the same property). `tool_input` is
    /// already secret-redacted upstream (itr#89).
    pub async fn recent_artifact_touches(
        &self,
        since: &str,
        limit: u32,
    ) -> Result<Vec<(String, String, String, String, String)>> {
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT agent_id, project, tool_name, tool_input, resolved_at
             FROM decision_log
             WHERE decision = '\"approve\"'
               AND tool_name IN ('Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'Bash')
               AND resolved_at >= ?1
             ORDER BY resolved_at DESC
             LIMIT ?2",
        )
        .bind(since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use crate::state::test_support::{make_request, test_db};
    use wisphive_protocol::Decision;

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
    // recent_artifact_touches (itr#402 burn meter)
    // ════════════════════════════════════════════════════════════

    use crate::state::AutoApprovedEntry;

    /// Insert a decision_log row with a controlled resolved_at timestamp.
    async fn seed_row(
        db: &crate::state::StateDb,
        agent_id: &str,
        tool_name: &str,
        tool_input: &str,
        timestamp: &str,
        decision: &str,
    ) {
        let inserted = db
            .log_auto_approved(&AutoApprovedEntry {
                agent_id,
                agent_type: "\"claude_code\"",
                project: "/muse",
                tool_name,
                tool_input,
                timestamp,
                tool_use_id: Some(&format!("{agent_id}-{tool_name}-{timestamp}")),
                hook_event_name: Some("PreToolUse"),
                decision,
                decided_by: Some("level:all"),
                config_hash: None,
            })
            .await
            .unwrap();
        assert!(inserted);
    }

    #[tokio::test]
    async fn recent_artifact_touches_empty() {
        let db = test_db().await;
        let rows = db
            .recent_artifact_touches("2020-01-01T00:00:00Z", 10)
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn recent_artifact_touches_filters_tools_decisions_and_window() {
        let db = test_db().await;

        // In: approved artifact-candidate tools inside the window.
        seed_row(
            &db,
            "cc-1",
            "Write",
            r#"{"file_path": "/muse/a.rs"}"#,
            "2026-07-15T12:00:00Z",
            "approve",
        )
        .await;
        seed_row(
            &db,
            "cc-1",
            "Bash",
            r#"{"command": "git commit -m 'feat: x'"}"#,
            "2026-07-15T12:05:00Z",
            "approve",
        )
        .await;
        // Out: non-candidate tool (Read is spend, never an artifact row).
        seed_row(&db, "cc-1", "Read", "{}", "2026-07-15T12:01:00Z", "approve").await;
        // Out: denied call — a denied Write produced nothing.
        seed_row(
            &db,
            "cc-1",
            "Write",
            r#"{"file_path": "/muse/b.rs"}"#,
            "2026-07-15T12:02:00Z",
            "deny",
        )
        .await;
        // Out: before the window.
        seed_row(
            &db,
            "cc-1",
            "Edit",
            r#"{"file_path": "/muse/old.rs"}"#,
            "2026-07-15T10:00:00Z",
            "approve",
        )
        .await;

        let rows = db
            .recent_artifact_touches("2026-07-15T11:30:00Z", 100)
            .await
            .unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
        // Newest first.
        assert_eq!(names, vec!["Bash", "Write"]);
        let (agent_id, project, _, tool_input, resolved_at) = rows[1].clone();
        assert_eq!(agent_id, "cc-1");
        assert_eq!(project, "/muse");
        assert_eq!(tool_input, r#"{"file_path": "/muse/a.rs"}"#);
        assert_eq!(resolved_at, "2026-07-15T12:00:00Z");
    }

    #[tokio::test]
    async fn recent_artifact_touches_respects_limit() {
        let db = test_db().await;
        for i in 0..5 {
            seed_row(
                &db,
                "cc-1",
                "Edit",
                r#"{"file_path": "/muse/x.rs"}"#,
                &format!("2026-07-15T12:0{i}:00Z"),
                "approve",
            )
            .await;
        }
        let rows = db
            .recent_artifact_touches("2026-07-15T00:00:00Z", 2)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        // The newest two survive the cap.
        assert_eq!(rows[0].4, "2026-07-15T12:04:00Z");
        assert_eq!(rows[1].4, "2026-07-15T12:03:00Z");
    }
}
