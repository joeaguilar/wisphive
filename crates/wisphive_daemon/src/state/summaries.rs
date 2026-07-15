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
}
