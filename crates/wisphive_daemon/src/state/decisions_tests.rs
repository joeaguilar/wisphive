use super::*;
use crate::state::test_support::{make_request, make_request_with_tool_use_id, test_db};
use wisphive_protocol::Decision;

/// Shorthand for tests: call log_auto_approved with positional args.
#[allow(clippy::too_many_arguments)]
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
        decision: "approve",
        decided_by: Some("level:all"),
        config_hash: None,
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
async fn persisted_rows_and_results_are_redacted() {
    // itr#89: secrets in tool_input/tool_result must never reach disk.
    let db = test_db().await;
    let mut req = make_request_with_tool_use_id("Bash", "cc-1", "sec-1");
    req.tool_input = serde_json::json!({"command": "export API_KEY=sk-abc123def456 && deploy"});
    let id = req.id;

    db.persist_pending(&req).await.unwrap();
    db.resolve_pending(id, Decision::Approve).await.unwrap();
    db.attach_tool_result(
        "cc-1",
        "Bash",
        &serde_json::json!({"output": "GITHUB_TOKEN=ghp_zzzzzzzzzzzz exported"}),
        Some("sec-1"),
    )
    .await
    .unwrap();

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    let dump = serde_json::to_string(&history[0]).unwrap();
    assert!(!dump.contains("sk-abc123"), "secret survived in tool_input");
    assert!(!dump.contains("ghp_zzzz"), "secret survived in tool_result");
    assert!(dump.contains("***REDACTED***"));
    // Non-secret context is preserved for the audit reader.
    assert!(dump.contains("deploy"));
}

#[tokio::test]
async fn persist_pending_never_overwrites_an_existing_row() {
    // itr#370: the id is hook-supplied; a colliding second request must not
    // rewrite the victim's persisted row (was INSERT OR REPLACE).
    let db = test_db().await;
    let victim = make_request("Bash", "cc-victim", "/muse");
    let id = victim.id;
    db.persist_pending(&victim).await.unwrap();

    let mut attacker = make_request("Write", "cc-attacker", "/evil");
    attacker.id = id;
    db.persist_pending(&attacker).await.unwrap();

    db.resolve_pending(id, Decision::Approve).await.unwrap();
    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].tool_name, "Bash", "victim's row must survive");
    assert_eq!(history[0].agent_id, "cc-victim");
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
    db.resolve_pending(fake_id, Decision::Approve)
        .await
        .unwrap();
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
// pending_decisions cluster (itr#298 / #299 / #300)
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn ask_defer_removes_pending_row_without_logging() {
    // itr#298: an Ask/defer must delete the pending row (else it leaks) but
    // must NOT write a decision_log entry (Ask is not a terminal decision).
    let db = test_db().await;
    let req = make_request("Bash", "cc-1", "/muse");
    let id = req.id;

    db.persist_pending(&req).await.unwrap();
    assert_eq!(db.pending_count().await.unwrap(), 1);

    db.delete_pending(id).await.unwrap();

    assert_eq!(
        db.pending_count().await.unwrap(),
        0,
        "pending row must be gone"
    );
    assert!(
        db.query_history(None, 10).await.unwrap().is_empty(),
        "Ask/defer must not land in decision_log"
    );
    // Idempotent — deleting again is a no-op.
    db.delete_pending(id).await.unwrap();
}

#[tokio::test]
async fn drain_orphaned_pending_records_failopen_and_clears_table() {
    // itr#299: on restart, orphaned pending rows are recorded as the truthful
    // fail-open Approve (the hook already ran the tool) and the table emptied.
    let db = test_db().await;
    let a = make_request("Bash", "cc-a", "/muse");
    let b = make_request("Write", "cc-b", "/muse");
    db.persist_pending(&a).await.unwrap();
    db.persist_pending(&b).await.unwrap();
    assert_eq!(db.pending_count().await.unwrap(), 2);

    let drained = db.drain_orphaned_pending().await.unwrap();
    assert_eq!(drained, 2);
    assert_eq!(
        db.pending_count().await.unwrap(),
        0,
        "table must be cleared"
    );

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 2);
    for entry in &history {
        assert_eq!(
            entry.decision,
            Decision::Approve,
            "fail-open outcome, not Deny"
        );
        assert_eq!(entry.decided_by.as_deref(), Some("daemon_restart:failopen"));
    }

    // Idempotent: a second drain finds nothing and adds no rows.
    assert_eq!(db.drain_orphaned_pending().await.unwrap(), 0);
    assert_eq!(db.query_history(None, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn drain_orphaned_spawn_is_failclosed_while_hook_rows_remain_failopen() {
    let db = test_db().await;
    let hook = make_request("Bash", "cc-a", "/muse");
    let hook_named_spawn = make_request("SpawnAgent", "cc-hook", "/muse");
    let mut spawn = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
    spawn.tool_input = serde_json::json!({
        "agent_type": "claude_code",
        "project": "/muse",
        "prompt": "noop",
    });
    db.persist_pending(&hook).await.unwrap();
    db.persist_pending(&hook_named_spawn).await.unwrap();
    db.persist_pending(&spawn).await.unwrap();

    assert_eq!(db.drain_orphaned_pending().await.unwrap(), 3);

    let history = db.query_history(None, 10).await.unwrap();
    let hook_entry = history.iter().find(|entry| entry.id == hook.id).unwrap();
    assert_eq!(hook_entry.decision, Decision::Approve);
    assert_eq!(
        hook_entry.decided_by.as_deref(),
        Some("daemon_restart:failopen")
    );
    let hook_named_spawn_entry = history
        .iter()
        .find(|entry| entry.id == hook_named_spawn.id)
        .unwrap();
    assert_eq!(
        hook_named_spawn_entry.decision,
        Decision::Approve,
        "a real hook tool with the same name still follows hook fail-open semantics"
    );
    let spawn_entry = history.iter().find(|entry| entry.id == spawn.id).unwrap();
    assert_eq!(
        spawn_entry.decision,
        Decision::Deny,
        "an unexecuted managed spawn must never be fabricated as approved"
    );
    assert_eq!(
        spawn_entry.decided_by.as_deref(),
        Some("daemon_restart:failclosed_spawn")
    );
}

#[tokio::test]
async fn approved_spawn_audit_uses_complete_reviewed_request() {
    let db = test_db().await;
    let mut pending = make_request("SpawnAgent", "wisphive-daemon:spawn", "/original");
    pending.tool_input = serde_json::json!({
        "agent_type": "claude_code",
        "project": "/original",
        "prompt": "original prompt",
    });
    db.persist_pending(&pending).await.unwrap();
    let reviewed: wisphive_protocol::SpawnAgentRequest =
        serde_json::from_value(serde_json::json!({
            "agent_type": "codex",
            "project": "/reviewed",
            "prompt": "reviewed prompt",
            "model": "gpt-5-codex",
            "reasoning": "high",
            "output_format": "json",
        }))
        .unwrap();

    assert!(
        db.resolve_spawn_pending_by(pending.id, &reviewed, "human:tui")
            .await
            .unwrap()
    );

    assert_eq!(db.pending_count().await.unwrap(), 0);
    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].agent_type, wisphive_protocol::AgentType::Codex);
    assert_eq!(history[0].project, std::path::PathBuf::from("/reviewed"));
    assert_eq!(history[0].tool_input["prompt"], "reviewed prompt");
    assert_eq!(history[0].decision, Decision::Approve);
    assert_eq!(history[0].decided_by.as_deref(), Some("human:tui"));
}

#[tokio::test]
async fn failclosed_spawn_reconciliation_handles_pending_and_committed_approval() {
    let db = test_db().await;

    let mut pending = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
    pending.tool_input = serde_json::json!({
        "agent_type": "claude_code",
        "project": "/muse",
        "prompt": "pending",
    });
    db.persist_pending(&pending).await.unwrap();
    assert!(
        db.force_failclosed_spawn_resolution(
            pending.id,
            "spawn_persistence_failure:deny",
            &pending,
        )
        .await
        .unwrap()
    );

    let mut approved = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
    approved.tool_input = pending.tool_input.clone();
    db.persist_pending(&approved).await.unwrap();
    let reviewed: wisphive_protocol::SpawnAgentRequest =
        serde_json::from_value(approved.tool_input.clone()).unwrap();
    assert!(
        db.resolve_spawn_pending_by(approved.id, &reviewed, "human:web")
            .await
            .unwrap()
    );
    assert!(
        db.force_failclosed_spawn_resolution(
            approved.id,
            "spawn_persistence_failure:deny",
            &approved,
        )
        .await
        .unwrap()
    );

    let mut deleted = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
    deleted.tool_input = pending.tool_input.clone();
    db.persist_pending(&deleted).await.unwrap();
    db.delete_pending(deleted.id).await.unwrap();
    assert!(
        db.force_failclosed_spawn_resolution(
            deleted.id,
            "spawn_persistence_failure:deny",
            &deleted,
        )
        .await
        .unwrap()
    );

    assert_eq!(db.pending_count().await.unwrap(), 0);
    let history = db.query_history(None, 10).await.unwrap();
    for id in [pending.id, approved.id, deleted.id] {
        let entry = history.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(entry.decision, Decision::Deny);
        assert_eq!(
            entry.decided_by.as_deref(),
            Some("spawn_persistence_failure:deny")
        );
    }
}

#[tokio::test]
async fn approved_spawn_action_failure_is_durable_and_redacted() {
    let db = test_db().await;
    let mut pending = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
    pending.tool_input = serde_json::json!({
        "agent_type": "claude_code",
        "project": "/muse",
        "prompt": "approved",
    });
    db.persist_pending(&pending).await.unwrap();
    let reviewed: wisphive_protocol::SpawnAgentRequest =
        serde_json::from_value(pending.tool_input.clone()).unwrap();
    assert!(
        db.resolve_spawn_pending_by(pending.id, &reviewed, "human:tui")
            .await
            .unwrap()
    );

    assert!(
        db.record_spawn_action_failure(pending.id, "missing sk-secret123456789")
            .await
            .unwrap()
    );
    let history = db.query_history(None, 10).await.unwrap();
    let entry = &history[0];
    assert_eq!(entry.decision, Decision::Approve);
    assert_eq!(
        entry.decided_by.as_deref(),
        Some("human:tui:spawn_action_failed")
    );
    let result = entry.tool_result.as_ref().unwrap();
    assert_eq!(result["spawn_status"], "action_failed");
    assert_ne!(result["error"], "missing sk-secret123456789");
}

#[tokio::test]
async fn persist_pending_leaves_permission_suggestions_null() {
    // itr#300 (resolved by #299): suggestions are intentionally not persisted —
    // pending_decisions is drained, not re-served, so there is no read model to
    // feed, and persisting them risked a cleartext-secret leak (itr#89). The
    // column stays NULL even when the request carries suggestions.
    use wisphive_protocol::{PermissionRule, PermissionSuggestion};
    let db = test_db().await;
    let mut req = make_request("Bash", "cc-1", "/muse");
    req.permission_suggestions = Some(vec![PermissionSuggestion {
        suggestion_type: "addRules".into(),
        rules: vec![PermissionRule {
            tool_name: "Bash".into(),
            // A secret in a rule_content must never reach disk here.
            rule_content: "curl -H 'Authorization: Bearer sk-leak12345'".into(),
        }],
        behavior: "allow".into(),
        destination: "session".into(),
        mode: None,
    }]);
    let id = req.id;

    db.persist_pending(&req).await.unwrap();

    let stored: (Option<String>,) =
        sqlx::query_as("SELECT permission_suggestions FROM pending_decisions WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(stored.0.is_none(), "suggestions column must be NULL");
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
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("tui-1"),
        Some("PreToolUse"),
    )
    .await;

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].tool_name, "Read");
    assert_eq!(history[0].decision, Decision::Approve);
}

#[tokio::test]
async fn log_auto_approved_dedup_with_tool_use_id() {
    let db = test_db().await;

    // Insert same event twice with same tool_use_id
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("tui-1"),
        Some("PreToolUse"),
    )
    .await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("tui-1"),
        Some("PreToolUse"),
    )
    .await;

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(
        history.len(),
        1,
        "duplicate with same tool_use_id should be ignored"
    );
}

/// Fixed #58: Events without tool_use_id are now deduplicated via
/// deterministic content-hashed IDs in log_auto_approved().
#[tokio::test]
async fn log_auto_approved_dedup_without_tool_use_id() {
    let db = test_db().await;

    // Insert same event twice with NO tool_use_id
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        None,
        Some("PreToolUse"),
    )
    .await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        None,
        Some("PreToolUse"),
    )
    .await;

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(
        history.len(),
        1,
        "deterministic IDs should deduplicate events without tool_use_id"
    );
}

#[tokio::test]
async fn recent_audit_decisions_returns_auto_and_deferred_not_human_denies() {
    let db = test_db().await;

    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Read",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("auto-1"),
        Some("PreToolUse"),
    )
    .await;

    db.log_auto_approved(&AutoApprovedEntry {
        agent_id: "cc-2",
        agent_type: "\"claude_code\"",
        project: "/muse",
        tool_name: "AskUserQuestion",
        tool_input: "{}",
        timestamp: "2024-01-01T00:01:00Z",
        tool_use_id: Some("defer-1"),
        hook_event_name: Some("PreToolUse"),
        decision: "ask",
        decided_by: Some("always_ask:intrinsic"),
        config_hash: None,
    })
    .await
    .unwrap();

    let req = make_request("Bash", "cc-human", "/muse");
    db.persist_pending(&req).await.unwrap();
    db.resolve_pending_by(req.id, Decision::Deny, "human:tui")
        .await
        .unwrap();

    let since = chrono::DateTime::parse_from_rfc3339("2023-12-31T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let recent = db.recent_audit_decisions(since, 10).await.unwrap();

    assert_eq!(recent.len(), 2);
    assert_eq!(
        recent[0].kind,
        wisphive_protocol::AuditDecisionKind::Deferred
    );
    assert_eq!(
        recent[0].decided_by.as_deref(),
        Some("always_ask:intrinsic")
    );
    assert_eq!(
        recent[1].kind,
        wisphive_protocol::AuditDecisionKind::AutoApproved
    );
    assert_eq!(recent[1].decided_by.as_deref(), Some("level:all"));
}

#[tokio::test]
async fn deferred_row_attach_flags_was_deferred_and_resolved() {
    // itr#461: answering a deferred native prompt in the terminal arrives as a
    // PostToolUse ToolResult. attach_tool_result must flag the matched row as a
    // deferral (so the server broadcasts DeferredResolved), and a subsequent
    // snapshot must mark that row resolved so a reconnect does not re-show it.
    let db = test_db().await;

    db.log_auto_approved(&AutoApprovedEntry {
        agent_id: "cc-2",
        agent_type: "\"claude_code\"",
        project: "/muse",
        tool_name: "AskUserQuestion",
        tool_input: "{}",
        timestamp: "2024-01-01T00:01:00Z",
        tool_use_id: Some("defer-42"),
        hook_event_name: Some("PreToolUse"),
        decision: "ask",
        decided_by: Some("always_ask:intrinsic"),
        config_hash: None,
    })
    .await
    .unwrap();

    let since = chrono::DateTime::parse_from_rfc3339("2023-12-31T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Before answering: the deferral is surfaced as NOT resolved.
    let before = db.recent_audit_decisions(since, 10).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].resolved, Some(false));
    assert_eq!(before[0].tool_use_id.as_deref(), Some("defer-42"));

    // The answer arrives (exact tool_use_id match); the row is a deferral.
    let answer = serde_json::json!({"answers": {"Greeting?": "Hey there!"}});
    let matched = db
        .attach_tool_result("cc-2", "AskUserQuestion", &answer, Some("defer-42"))
        .await
        .unwrap()
        .expect("deferred row should match");
    assert!(matched.was_deferred);

    // After answering: the same row is surfaced as resolved.
    let after = db.recent_audit_decisions(since, 10).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].resolved, Some(true));
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
    let matched = db
        .attach_tool_result("cc-1", "Bash", &result, Some("tui-456"))
        .await
        .unwrap();
    assert!(matched.is_some());
    let matched = matched.unwrap();
    assert_eq!(matched.id, id);
    // An approved (non-deferred) row must not be flagged as a deferral.
    assert!(!matched.was_deferred);

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
    let matched = db
        .attach_tool_result("cc-1", "Bash", &result, None)
        .await
        .unwrap();
    assert!(matched.is_some());
}

#[tokio::test]
async fn attach_tool_result_no_match() {
    let db = test_db().await;
    let result = serde_json::json!({"output": "orphan"});
    let matched = db
        .attach_tool_result("cc-99", "Bash", &result, None)
        .await
        .unwrap();
    assert!(matched.is_none());
}

#[tokio::test]
async fn attach_tool_result_does_not_overwrite() {
    let db = test_db().await;
    let req = make_request_with_tool_use_id("Bash", "cc-1", "tui-789");
    db.persist_pending(&req).await.unwrap();
    db.resolve_pending(req.id, Decision::Approve).await.unwrap();

    let r1 = serde_json::json!({"output": "first"});
    db.attach_tool_result("cc-1", "Bash", &r1, Some("tui-789"))
        .await
        .unwrap();

    // Second attach to same tool_use_id should find no match (already has result)
    let r2 = serde_json::json!({"output": "second"});
    let matched = db
        .attach_tool_result("cc-1", "Bash", &r2, Some("tui-789"))
        .await
        .unwrap();
    assert!(
        matched.is_none(),
        "should not overwrite existing tool_result"
    );
}

// ════════════════════════════════════════════════════════════
// search_history
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn search_history_matches_partial_token_substring() {
    let db = test_db().await;

    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{\"command\":\"cargo build\"}",
        "2024-01-01T00:00:00Z",
        Some("a"),
        None,
    )
    .await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Edit",
        "{\"file\":\"main.rs\"}",
        "2024-01-01T00:01:00Z",
        Some("b"),
        None,
    )
    .await;

    for query in ["carg", "arg"] {
        let search = wisphive_protocol::HistorySearch {
            query: Some(query.into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(results.len(), 1, "substring {query:?} must match cargo");
        assert_eq!(results[0].tool_name, "Bash");
    }
}

#[tokio::test]
async fn search_history_sql_shaped_query_is_literal() {
    let db = test_db().await;

    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{\"command\":\"cargo build\"}",
        "2024-01-01T00:00:00Z",
        Some("safe"),
        None,
    )
    .await;
    log_auto(
        &db,
        "cc-2",
        "\"claude_code\"",
        "/muse",
        "Edit",
        "{\"file\":\"main.rs\"}",
        "2024-01-01T00:01:00Z",
        Some("other"),
        None,
    )
    .await;

    for query in ["cargo' OR 1=1 --", "cargo\" OR \"main"] {
        let search = wisphive_protocol::HistorySearch {
            query: Some(query.into()),
            ..Default::default()
        };
        assert!(
            db.search_history(&search).await.unwrap().is_empty(),
            "SQL/FTS-shaped text must remain a literal bound substring: {query:?}"
        );
    }
}

#[tokio::test]
async fn search_history_empty_or_whitespace_query_preserves_other_filters() {
    let db = test_db().await;

    for (agent_id, tool_name, timestamp, tool_use_id) in [
        ("cc-1", "Bash", "2024-01-01T00:00:00Z", "empty-a"),
        ("cc-1", "Edit", "2024-01-01T00:01:00Z", "empty-b"),
        ("cc-2", "Write", "2024-01-01T00:02:00Z", "empty-c"),
    ] {
        log_auto(
            &db,
            agent_id,
            "\"claude_code\"",
            "/muse",
            tool_name,
            "{}",
            timestamp,
            Some(tool_use_id),
            None,
        )
        .await;
    }

    for query in ["", " \t\n ", "---"] {
        let search = wisphive_protocol::HistorySearch {
            query: Some(query.into()),
            agent_id: Some("cc-1".into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(
            results.len(),
            2,
            "no-term query {query:?} must omit only the text predicate"
        );
        assert!(results.iter().all(|entry| entry.agent_id == "cc-1"));
    }
}

#[tokio::test]
async fn search_history_short_query_falls_back_without_error() {
    let db = test_db().await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{\"command\":\"cargo build\"}",
        "2024-01-01T00:00:00Z",
        Some("short"),
        None,
    )
    .await;

    for query in ["c", "ca", "ar"] {
        let search = wisphive_protocol::HistorySearch {
            query: Some(query.into()),
            ..Default::default()
        };
        let results = db.search_history(&search).await.unwrap();
        assert_eq!(results.len(), 1, "short substring {query:?} must match");
        assert_eq!(results[0].tool_name, "Bash");
    }
}

#[tokio::test]
async fn search_history_nul_query_falls_back_without_error() {
    let db = test_db().await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{\"command\":\"cargo build\"}",
        "2024-01-01T00:00:00Z",
        Some("nul"),
        None,
    )
    .await;

    let search = wisphive_protocol::HistorySearch {
        query: Some("car\0go".into()),
        ..Default::default()
    };
    assert!(
        db.search_history(&search).await.unwrap().is_empty(),
        "embedded NUL must remain literal and must not reach the FTS parser"
    );
}

#[tokio::test]
async fn search_history_fts_mirror_tracks_tool_result_updates_and_deletes() {
    let db = test_db().await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{\"command\":\"build project\"}",
        "2024-01-01T00:00:00Z",
        Some("result-index"),
        None,
    )
    .await;

    db.attach_tool_result(
        "cc-1",
        "Bash",
        &serde_json::json!({"output": "cargo build completed"}),
        Some("result-index"),
    )
    .await
    .unwrap();

    let search = wisphive_protocol::HistorySearch {
        query: Some("carg".into()),
        ..Default::default()
    };
    let results = db.search_history(&search).await.unwrap();
    assert_eq!(results.len(), 1, "updated tool_result must enter FTS");

    sqlx::query("DELETE FROM decision_log WHERE id = ?")
        .bind(results[0].id.to_string())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        db.search_history(&search).await.unwrap().is_empty(),
        "deleting a decision_log row must remove its FTS entry"
    );
}

#[tokio::test]
async fn search_history_100k_substring_completes_under_50ms() {
    let db = test_db().await;
    sqlx::query(
        "WITH RECURSIVE rows(n) AS (
             SELECT 1
             UNION ALL
             SELECT n + 1 FROM rows WHERE n < 100000
         )
         INSERT INTO decision_log (
             id, agent_id, agent_type, project, tool_name, tool_input,
             decision, requested_at, resolved_at
         )
         SELECT
             printf('00000000-0000-0000-0000-%012d', n),
             'perf-agent',
             '\"claude_code\"',
             '/perf',
             CASE WHEN n = 54321 THEN 'Bash' ELSE 'Edit' END,
             CASE WHEN n = 54321
                  THEN '{\"command\":\"cargo build target\"}'
                  ELSE printf('{\"file\":\"module-%d.rs\",\"operation\":\"replace symbols\"}', n)
             END,
             '\"approve\"',
             '2024-01-01T00:00:00Z',
             '2024-01-01T00:00:00Z'
         FROM rows",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let search = wisphive_protocol::HistorySearch {
        query: Some("carg".into()),
        limit: Some(10),
        ..Default::default()
    };
    assert_eq!(db.search_history(&search).await.unwrap().len(), 1);

    let mut samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let results = db.search_history(&search).await.unwrap();
        samples.push(started.elapsed());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "Bash");
    }
    samples.sort_unstable();
    assert!(
        samples[2] < std::time::Duration::from_millis(50),
        "median 100k-row substring latency must be <50ms; samples={samples:?}"
    );
}

#[tokio::test]
async fn search_history_migration_replaces_legacy_fts_and_backfills() {
    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("wisphive.db");
    let database_path = database_path.to_string_lossy().into_owned();
    let db = StateDb::open(&database_path).await.unwrap();
    for statement in [
        "DROP TRIGGER IF EXISTS decision_log_fts_after_insert",
        "DROP TRIGGER IF EXISTS decision_log_fts_after_delete",
        "DROP TRIGGER IF EXISTS decision_log_fts_after_update",
        "DROP TABLE IF EXISTS decision_log_fts",
        "DELETE FROM wisphive_schema_migrations WHERE name = 'decision_log_fts_trigram_v1'",
    ] {
        sqlx::query(statement).execute(db.pool()).await.unwrap();
    }

    sqlx::query(
        "CREATE VIRTUAL TABLE decision_log_fts USING fts5(
             tool_input,
             tool_result,
             tool_name,
             content='decision_log',
             content_rowid='rowid',
             tokenize='unicode61'
         )",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO decision_log (
             id, agent_id, agent_type, project, tool_name, tool_input,
             decision, requested_at, resolved_at
         ) VALUES (
             '00000000-0000-0000-0000-000000000091',
             'migration-agent',
             '\"claude_code\"',
             '/migration',
             'Bash',
             '{\"command\":\"cargo build\"}',
             '\"approve\"',
             '2024-01-01T00:00:00Z',
             '2024-01-01T00:00:00Z'
         )",
    )
    .execute(db.pool())
    .await
    .unwrap();

    db.migrate().await.unwrap();

    let (schema,): (String,) = sqlx::query_as(
        "SELECT sql FROM sqlite_schema
         WHERE type = 'table' AND name = 'decision_log_fts'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(
        schema.contains("tokenize='trigram'"),
        "legacy tokenizer was not replaced: {schema}"
    );

    let search = wisphive_protocol::HistorySearch {
        query: Some("carg".into()),
        ..Default::default()
    };
    assert_eq!(
        db.search_history(&search).await.unwrap().len(),
        1,
        "the one-time rebuild must index pre-migration rows"
    );

    // Deliberately empty only the FTS index. A second O(n) rebuild would make
    // this row searchable again; a marker-gated steady boot must leave it
    // untouched. Reopen a file-backed DB to prove that marker is durable.
    sqlx::query("INSERT INTO decision_log_fts(decision_log_fts) VALUES ('delete-all')")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.search_history(&search).await.unwrap().is_empty());
    db.pool().close().await;
    drop(db);

    let reopened = StateDb::open(&database_path).await.unwrap();
    assert!(
        reopened.search_history(&search).await.unwrap().is_empty(),
        "steady-state reopen must not rescan decision_log"
    );
    let (markers,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM wisphive_schema_migrations
         WHERE name = 'decision_log_fts_trigram_v1'",
    )
    .fetch_one(reopened.pool())
    .await
    .unwrap();
    assert_eq!(markers, 1, "the backfill marker must remain durable");
}

#[tokio::test]
async fn search_history_by_tool_name() {
    let db = test_db().await;

    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("a"),
        None,
    )
    .await;
    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Edit",
        "{}",
        "2024-01-01T00:01:00Z",
        Some("b"),
        None,
    )
    .await;

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

    log_auto(
        &db,
        "cc-1",
        "\"claude_code\"",
        "/muse",
        "Bash",
        "{}",
        "2024-01-01T00:00:00Z",
        Some("a"),
        None,
    )
    .await;
    log_auto(
        &db,
        "cc-2",
        "\"claude_code\"",
        "/rpg",
        "Bash",
        "{}",
        "2024-01-01T00:01:00Z",
        Some("b"),
        None,
    )
    .await;

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
