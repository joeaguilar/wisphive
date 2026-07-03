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
