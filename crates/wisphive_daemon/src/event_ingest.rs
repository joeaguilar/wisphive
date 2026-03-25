use std::path::PathBuf;
use std::sync::Arc;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::state::StateDb;

/// Spawn an async task that tails `events.jsonl` and batch-inserts auto-approved
/// events into the decision_log.
///
/// Uses the `notify` crate for file change detection. On each modify event,
/// reads new lines from a tracked byte offset, parses JSON, and inserts into
/// SQLite with `auto_approved = 1`.
pub fn spawn_event_ingest(
    events_path: PathBuf,
    state_db: Arc<StateDb>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_ingest(events_path, state_db).await {
            error!("event ingest task failed: {e}");
        }
    })
}

async fn run_ingest(events_path: PathBuf, state_db: Arc<StateDb>) -> anyhow::Result<()> {
    // Create the events file if it doesn't exist
    if !events_path.exists() {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path);
    }

    // Channel for notify events → async task
    let (tx, mut rx) = mpsc::channel::<()>(64);

    // Set up file watcher
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res
            && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = tx.try_send(());
            }
    })?;

    // Watch the parent directory (file may not exist yet at startup)
    let watch_dir = events_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

    info!(path = %events_path.display(), "event ingest watching");

    // Open the file and seek to end (only process new events)
    let file = tokio::fs::File::open(&events_path).await?;
    let mut reader = BufReader::new(file);
    reader.seek(std::io::SeekFrom::End(0)).await?;

    let mut line_buf = String::new();

    loop {
        // Wait for file change notification
        if rx.recv().await.is_none() {
            break; // Channel closed, shutdown
        }

        // Drain any extra notifications that queued up
        while rx.try_recv().is_ok() {}

        // Read all new lines
        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => break, // No more data
                Ok(_) => {
                    let trimmed = line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Err(e) = ingest_line(trimmed, &state_db).await {
                        warn!("failed to ingest event line: {e}");
                    }
                }
                Err(e) => {
                    warn!("error reading events.jsonl: {e}");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Read all lines from events.jsonl and ingest them into the database.
/// Returns the number of events successfully ingested.
/// Uses INSERT OR IGNORE with a unique index on tool_use_id for deduplication.
pub async fn reimport_all(events_path: &std::path::Path, state_db: &StateDb) -> anyhow::Result<u64> {
    use tokio::io::AsyncBufReadExt;

    if !events_path.exists() {
        return Ok(0);
    }

    let file = tokio::fs::File::open(events_path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut count = 0u64;

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if ingest_line(trimmed, state_db).await.is_ok() {
            count += 1;
        }
    }

    info!(count, "reimported events from events.jsonl");
    Ok(count)
}

/// Parse a single JSONL line and insert into decision_log as auto-approved.
pub async fn ingest_line(line: &str, state_db: &StateDb) -> anyhow::Result<()> {
    let event: serde_json::Value = serde_json::from_str(line)?;

    let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
    if event_type != "auto_approved" {
        debug!(event_type, "skipping non-auto-approved event");
        return Ok(());
    }

    let agent_id = event
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let agent_type = event
        .get("agent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("claude_code");
    let project = event
        .get("project")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_name = event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let tool_input = event
        .get("tool_input")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "{}".to_string());
    let timestamp = event
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_use_id = event
        .get("tool_use_id")
        .and_then(|v| v.as_str());
    let hook_event_name = event
        .get("hook_event_name")
        .and_then(|v| v.as_str());

    // Serialize agent_type as JSON string to match existing format
    let agent_type_json = format!("\"{}\"", agent_type);

    state_db
        .log_auto_approved(&crate::state::AutoApprovedEntry {
            agent_id,
            agent_type: &agent_type_json,
            project,
            tool_name,
            tool_input: &tool_input,
            timestamp,
            tool_use_id,
            hook_event_name,
        })
        .await?;

    debug!(tool_name, agent_id, "ingested auto-approved event");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateDb;

    async fn test_db() -> StateDb {
        StateDb::open(":memory:").await.unwrap()
    }

    fn auto_approved_event(tool: &str, agent_id: &str, tool_use_id: Option<&str>) -> String {
        let mut event = serde_json::json!({
            "event": "auto_approved",
            "agent_id": agent_id,
            "agent_type": "claude_code",
            "project": "/test",
            "tool_name": tool,
            "tool_input": {"command": "test"},
            "timestamp": "2024-01-01T00:00:00Z",
        });
        if let Some(tui) = tool_use_id {
            event["tool_use_id"] = serde_json::Value::String(tui.into());
        }
        serde_json::to_string(&event).unwrap()
    }

    // ════════════════════════════════════════════════════════════
    // ingest_line
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn ingest_auto_approved_event() {
        let db = test_db().await;
        let line = auto_approved_event("Bash", "cc-1", Some("tui-1"));
        ingest_line(&line, &db).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "Bash");
        assert_eq!(history[0].agent_id, "cc-1");
    }

    #[tokio::test]
    async fn ingest_skips_non_auto_approved() {
        let db = test_db().await;
        let line = r#"{"event": "session_start", "agent_id": "cc-1"}"#;
        ingest_line(line, &db).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert!(history.is_empty(), "non-auto_approved events should be skipped");
    }

    #[tokio::test]
    async fn ingest_invalid_json_returns_error() {
        let db = test_db().await;
        let result = ingest_line("not json at all", &db).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ingest_missing_fields_uses_defaults() {
        let db = test_db().await;
        // Event with valid timestamp but missing other fields
        let line = r#"{"event": "auto_approved", "timestamp": "2024-01-01T00:00:00Z", "tool_use_id": "default-test"}"#;
        ingest_line(line, &db).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "unknown");
        assert_eq!(history[0].agent_id, "unknown");
    }

    #[tokio::test]
    async fn ingest_with_hook_event_name() {
        let db = test_db().await;
        let line = r#"{"event": "auto_approved", "agent_id": "cc-1", "tool_name": "Read", "tool_input": {}, "timestamp": "2024-01-01T00:00:00Z", "tool_use_id": "t1", "hook_event_name": "PreToolUse"}"#;
        ingest_line(line, &db).await.unwrap();

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].hook_event_name, Some("PreToolUse".to_string()));
    }

    // ════════════════════════════════════════════════════════════
    // reimport_all
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn reimport_all_from_file() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let content = format!(
            "{}\n{}\n{}\n",
            auto_approved_event("Bash", "cc-1", Some("t1")),
            auto_approved_event("Edit", "cc-1", Some("t2")),
            auto_approved_event("Write", "cc-2", Some("t3")),
        );
        std::fs::write(&events_path, &content).unwrap();

        let count = reimport_all(&events_path, &db).await.unwrap();
        assert_eq!(count, 3);

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 3);
    }

    #[tokio::test]
    async fn reimport_all_nonexistent_file_returns_zero() {
        let db = test_db().await;
        let count = reimport_all(std::path::Path::new("/tmp/nonexistent_wisphive_test.jsonl"), &db)
            .await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn reimport_all_dedup_with_tool_use_id() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        // Same event repeated 3 times with same tool_use_id
        let event = auto_approved_event("Bash", "cc-1", Some("t1"));
        let content = format!("{event}\n{event}\n{event}\n");
        std::fs::write(&events_path, &content).unwrap();

        let count = reimport_all(&events_path, &db).await.unwrap();
        // All 3 lines "succeed" (INSERT OR IGNORE doesn't error)
        assert_eq!(count, 3);

        // But only 1 row should be in the DB thanks to dedup
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1, "duplicate tool_use_id events should be deduplicated");
    }

    /// Fixed #58: reimport_all no longer creates duplicates for events without tool_use_id.
    /// Deterministic content-hashed IDs ensure repeated reimports are idempotent.
    #[tokio::test]
    async fn reimport_all_dedup_without_tool_use_id() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        // Event without tool_use_id
        let event = auto_approved_event("Bash", "cc-1", None);
        std::fs::write(&events_path, format!("{event}\n")).unwrap();

        // First reimport
        reimport_all(&events_path, &db).await.unwrap();
        let after_first = db.query_history(None, 100).await.unwrap();
        assert_eq!(after_first.len(), 1);

        // Second reimport (simulates pressing Refresh)
        reimport_all(&events_path, &db).await.unwrap();
        let after_second = db.query_history(None, 100).await.unwrap();

        assert_eq!(after_second.len(), 1, "reimport should be idempotent for events without tool_use_id");
    }

    #[tokio::test]
    async fn reimport_all_skips_blank_lines() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let event = auto_approved_event("Bash", "cc-1", Some("t1"));
        let content = format!("\n\n{event}\n\n\n");
        std::fs::write(&events_path, &content).unwrap();

        let count = reimport_all(&events_path, &db).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn reimport_all_skips_non_auto_approved_events() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let content = format!(
            "{}\n{}\n{}\n",
            auto_approved_event("Bash", "cc-1", Some("t1")),
            r#"{"event": "session_start", "agent_id": "cc-1"}"#,
            r#"{"event": "notification", "text": "hi"}"#,
        );
        std::fs::write(&events_path, &content).unwrap();

        // reimport_all counts all lines where ingest_line returns Ok,
        // including skipped non-auto-approved events (they return Ok too).
        let count = reimport_all(&events_path, &db).await.unwrap();
        assert_eq!(count, 3, "all valid JSON lines return Ok from ingest_line");

        // But only the auto_approved event should be in the DB
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1, "only auto_approved events should be in the DB");
        assert_eq!(history[0].tool_name, "Bash");
    }
}
