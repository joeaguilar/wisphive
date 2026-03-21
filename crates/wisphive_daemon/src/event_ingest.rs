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
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = tx.try_send(());
            }
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

/// Parse a single JSONL line and insert into decision_log as auto-approved.
async fn ingest_line(line: &str, state_db: &StateDb) -> anyhow::Result<()> {
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
        .log_auto_approved(
            agent_id,
            &agent_type_json,
            project,
            tool_name,
            &tool_input,
            timestamp,
            tool_use_id,
            hook_event_name,
        )
        .await?;

    debug!(tool_name, agent_id, "ingested auto-approved event");
    Ok(())
}
