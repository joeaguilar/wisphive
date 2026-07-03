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

    assert_eq!(db.pending_count().await.unwrap(), 0, "pending row must be gone");
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
    assert_eq!(db.pending_count().await.unwrap(), 0, "table must be cleared");

    let history = db.query_history(None, 10).await.unwrap();
    assert_eq!(history.len(), 2);
    for entry in &history {
        assert_eq!(entry.decision, Decision::Approve, "fail-open outcome, not Deny");
        assert_eq!(entry.decided_by.as_deref(), Some("daemon_restart:failopen"));
    }

    // Idempotent: a second drain finds nothing and adds no rows.
    assert_eq!(db.drain_orphaned_pending().await.unwrap(), 0);
    assert_eq!(db.query_history(None, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn persist_pending_round_trips_permission_suggestions() {
    // itr#300: PermissionRequest suggestions must persist into the column and
    // read back, not be silently dropped.
    use wisphive_protocol::{PermissionRule, PermissionSuggestion};
    let db = test_db().await;
    let mut req = make_request("Bash", "cc-1", "/muse");
    req.permission_suggestions = Some(vec![
        PermissionSuggestion {
            suggestion_type: "addRules".into(),
            rules: vec![PermissionRule {
                tool_name: "Bash".into(),
                rule_content: "npm run build".into(),
            }],
            behavior: "allow".into(),
            destination: "session".into(),
            mode: None,
        },
        PermissionSuggestion {
            suggestion_type: "setMode".into(),
            rules: vec![],
            behavior: "deny".into(),
            destination: "localSettings".into(),
            mode: Some("plan".into()),
        },
    ]);
    let id = req.id;

    db.persist_pending(&req).await.unwrap();

    let read = db.pending_permission_suggestions(id).await.unwrap().unwrap();
    assert_eq!(read.len(), 2, "both suggestions must survive the round-trip");
    assert_eq!(read[0].suggestion_type, "addRules");
    assert_eq!(read[0].behavior, "allow");
    assert_eq!(read[1].mode.as_deref(), Some("plan"));

    // A row with no suggestions reads back as None (not an error).
    let plain = make_request("Read", "cc-2", "/muse");
    let plain_id = plain.id;
    db.persist_pending(&plain).await.unwrap();
    assert!(db.pending_permission_suggestions(plain_id).await.unwrap().is_none());
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
    assert_eq!(matched.unwrap(), id);

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
async fn search_history_by_query() {
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

    let search = wisphive_protocol::HistorySearch {
        query: Some("cargo".into()),
        ..Default::default()
    };
    let results = db.search_history(&search).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_name, "Bash");
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
