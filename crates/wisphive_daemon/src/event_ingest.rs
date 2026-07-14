use std::path::PathBuf;
use std::sync::Arc;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use wisphive_protocol::{AuditDecision, AuditDecisionKind, ServerMessage};

use crate::state::StateDb;

/// Size cap for `events.jsonl` before it is rotated. The hook only appends to
/// this file; the daemon (sole consumer) owns its lifecycle, so rotation
/// happens here once new lines are drained into SQLite.
const EVENTS_LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Spawn an async task that tails `events.jsonl` and batch-inserts auto-approved
/// events into the decision_log.
///
/// Uses the `notify` crate for file change detection. On each modify event,
/// reads new lines from a tracked byte offset, parses JSON, and inserts into
/// SQLite with `auto_approved = 1`. Detects truncation/rotation (file shrank
/// below the tracked offset) and reseeks to the start so ingestion never
/// silently stalls. Once new lines are drained and the file exceeds
/// [`EVENTS_LOG_MAX_BYTES`], rotates it into `log_dir` (reaped by
/// `logging::prune_old_files`) to bound unbounded growth.
pub fn spawn_event_ingest(
    events_path: PathBuf,
    log_dir: PathBuf,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_ingest(events_path, log_dir, state_db, tui_tx).await {
            error!("event ingest task failed: {e}");
        }
    })
}

/// Open `events.jsonl` for reading and seek to `offset` (clamped to EOF).
async fn open_reader_at(
    events_path: &std::path::Path,
    offset: u64,
) -> anyhow::Result<(BufReader<tokio::fs::File>, u64)> {
    let file = tokio::fs::File::open(events_path).await?;
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let start = offset.min(len);
    let mut reader = BufReader::new(file);
    reader.seek(std::io::SeekFrom::Start(start)).await?;
    Ok((reader, start))
}

/// `(device, inode)` identity of the file at `path`, or `None` if it can't be
/// stat'd. Used to detect that the file was replaced (rotated/recreated) even
/// when the replacement is the same size or larger — a case the byte-offset
/// shrink check alone would miss.
async fn file_identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.dev(), meta.ino()))
}

async fn run_ingest(
    events_path: PathBuf,
    log_dir: PathBuf,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
) -> anyhow::Result<()> {
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
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res
                && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
            {
                let _ = tx.try_send(());
            }
        })?;

    // Watch the parent directory (file may not exist yet at startup)
    let watch_dir = events_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

    info!(path = %events_path.display(), "event ingest watching");

    // Capture the length BEFORE the startup reimport: the tail reader seeks
    // here, so a line appended mid-reimport is never skipped (it is either
    // caught by the reimport or re-read by the tail — dedup makes the overlap
    // harmless).
    let start_len = tokio::fs::metadata(&events_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Ingest the backlog written while the daemon was down (itr#301). The hook
    // appends auto-approved events regardless of daemon state; seeking straight
    // to EOF silently dropped that backlog from the audit log. Idempotent via
    // the content-hash/tool_use_id dedup, so restarts never double-ingest.
    match reimport_all(&events_path, &state_db).await {
        Ok(count) if count > 0 => info!(count, "startup reimport of events.jsonl backlog"),
        Ok(_) => {}
        Err(e) => warn!("startup reimport of events.jsonl failed: {e}"),
    }

    // Tail from `start_len`; new lines arrive via notifications. `offset`
    // tracks our read position so we can detect truncation/rotation when the
    // file shrinks beneath it.
    let (mut reader, mut offset) = open_reader_at(&events_path, start_len).await?;
    let mut file_id = file_identity(&events_path).await;

    let mut line_buf = String::new();

    loop {
        // Wait for file change notification
        if rx.recv().await.is_none() {
            break; // Channel closed, shutdown
        }

        // Drain any extra notifications that queued up
        while rx.try_recv().is_ok() {}

        // Detect truncation/rotation: the file was replaced or truncated out
        // from under us if either (a) it is now shorter than our read position,
        // or (b) its (dev, inode) identity changed — catches a same-or-larger
        // replacement that the length check alone would miss. Reopen from the
        // start so we don't stall reading past EOF or skip a new file's prefix.
        // Re-ingest is harmless — ingest_line dedupes on (content-hashed) id.
        let current_len = tokio::fs::metadata(&events_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let current_id = file_identity(&events_path).await;
        if current_len < offset || current_id != file_id {
            match open_reader_at(&events_path, 0).await {
                Ok((r, o)) => {
                    reader = r;
                    offset = o;
                    file_id = current_id;
                }
                Err(e) => warn!("failed to reopen events.jsonl after rotation: {e}"),
            }
        }

        // Read all new lines
        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => break, // No more data
                Ok(n) => {
                    offset += n as u64;
                    let trimmed = line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match ingest_line(trimmed, &state_db).await {
                        Ok(Some(audit)) => {
                            let _ = tui_tx.send(ServerMessage::AuditDecision(audit));
                        }
                        Ok(None) => {}
                        Err(e) => warn!("failed to ingest event line: {e}"),
                    }
                }
                Err(e) => {
                    warn!("error reading events.jsonl: {e}");
                    break;
                }
            }
        }

        // Now that we are drained to EOF, rotate if the file is too large.
        if offset >= EVENTS_LOG_MAX_BYTES
            && let Some((reader_next, offset_next)) =
                rotate_events_log(&events_path, &log_dir, &state_db).await
        {
            reader = reader_next;
            offset = offset_next;
            file_id = file_identity(&events_path).await;
        }
    }

    Ok(())
}

/// Rotate `events.jsonl` into `log_dir` once it has been drained into SQLite.
///
/// Non-lossy: the file is renamed (so any line a hook appends mid-rotation lands
/// in the rotated segment, not lost), then `reimport_all` re-ingests the rotated
/// segment to catch stragglers (idempotent via dedup). The rotated segment lives
/// in `log_dir`, where `logging::prune_old_files` reaps it by age. Returns a
/// fresh reader/offset for the new (empty) `events.jsonl`, or `None` on failure
/// (in which case the caller keeps using the existing reader).
async fn rotate_events_log(
    events_path: &std::path::Path,
    log_dir: &std::path::Path,
    state_db: &StateDb,
) -> Option<(BufReader<tokio::fs::File>, u64)> {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f");
    let rotated = log_dir.join(format!("events-{stamp}.jsonl"));

    if let Err(e) = tokio::fs::rename(events_path, &rotated).await {
        warn!("failed to rotate events.jsonl: {e}");
        return None;
    }

    // Re-ingest the rotated segment to capture any lines appended between the
    // last drain and the rename. Dedup makes this safe. On failure the segment
    // is NOT auto-recovered (rotated segments are only age-reaped, never
    // re-read), so its un-ingested tail would be lost from SQLite — flag it as
    // `.failed` and escalate so an operator can re-import it. The file itself is
    // preserved on disk until `log_retention_days`.
    if let Err(e) = reimport_all(&rotated, state_db).await {
        let failed = rotated.with_extension("failed.jsonl");
        let kept = match tokio::fs::rename(&rotated, &failed).await {
            Ok(()) => failed,
            Err(_) => rotated.clone(),
        };
        error!(
            segment = %kept.display(),
            "failed to reimport rotated events segment; its un-ingested events are NOT in the DB (re-import manually): {e}"
        );
    }

    // Recreate an empty events.jsonl so the next hook append (and our reader)
    // have a file to work with, then reopen from the start.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path);

    match open_reader_at(events_path, 0).await {
        Ok((reader, offset)) => {
            info!(rotated = %rotated.display(), "rotated events.jsonl");
            Some((reader, offset))
        }
        Err(e) => {
            warn!("failed to reopen events.jsonl after rotation: {e}");
            None
        }
    }
}

/// Read all lines from events.jsonl and ingest them into the database.
/// Returns the number of events successfully ingested.
/// Uses INSERT OR IGNORE with a unique index on tool_use_id for deduplication.
pub async fn reimport_all(
    events_path: &std::path::Path,
    state_db: &StateDb,
) -> anyhow::Result<u64> {
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
        // A single malformed/torn JSON line must not abort the whole reimport
        // (a partial append is exactly what the tailing design tolerates). Skip
        // parse errors and keep going, but still propagate a genuine DB/systemic
        // error so the caller escalates the segment to `.failed.jsonl` (itr#336).
        match ingest_line(trimmed, state_db).await {
            Ok(Some(_audit)) => count += 1,
            Ok(None) => {}
            Err(e) if e.downcast_ref::<serde_json::Error>().is_some() => {
                warn!("skipping malformed event line during reimport: {e}");
            }
            Err(e) => return Err(e),
        }
    }

    info!(count, "reimported events from events.jsonl");
    Ok(count)
}

/// Re-import rotated event segments left behind by an interrupted prior run.
///
/// The startup caller runs this before log retention. Normal rotated segments
/// are usually already in `decision_log`, but a crash between rotation and its
/// re-import leaves them orphaned. Failed segments are explicitly retried; a
/// successful retry removes the `.failed.jsonl` recovery marker. Re-importing
/// is idempotent through `ingest_line`'s database-level deduplication.
///
/// If a normal segment cannot be imported, rename it to `.failed.jsonl` before
/// returning so retention cannot discard its only copy. A failed segment that
/// still cannot be imported is already protected from retention and remains in
/// place for the next startup attempt.
pub async fn reimport_rotated_segments(
    log_dir: &std::path::Path,
    state_db: &StateDb,
) -> anyhow::Result<u64> {
    let mut segments = Vec::new();
    for entry in std::fs::read_dir(log_dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!(
                    error = %err,
                    "skipping unreadable directory entry during rotated events recovery"
                );
                continue;
            }
        };
        let Ok(file_type) = entry.file_type() else {
            warn!(
                path = %entry.path().display(),
                "skipping unreadable entry during rotated events recovery"
            );
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_failed = name.starts_with("events-") && name.ends_with(".failed.jsonl");
        let is_normal = name.starts_with("events-") && name.ends_with(".jsonl") && !is_failed;
        if is_normal || is_failed {
            segments.push((entry.path(), is_failed));
        }
    }
    segments.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut imported = 0;
    for (segment, was_failed) in segments {
        match reimport_all(&segment, state_db).await {
            Ok(count) => {
                imported += count;
                if was_failed {
                    if let Err(e) = tokio::fs::remove_file(&segment).await {
                        warn!(
                            segment = %segment.display(),
                            "re-ingested failed events segment but could not reap it: {e}"
                        );
                    } else {
                        info!(segment = %segment.display(), count, "re-ingested and reaped failed events segment");
                    }
                } else if count > 0 {
                    info!(segment = %segment.display(), count, "re-imported rotated events segment");
                }
            }
            Err(e) if was_failed => {
                warn!(
                    segment = %segment.display(),
                    "failed events segment remains for a future startup re-import: {e}"
                );
            }
            Err(e) => {
                let failed = segment.with_extension("failed.jsonl");
                tokio::fs::rename(&segment, &failed).await.map_err(|rename_err| {
                    anyhow::anyhow!(
                        "failed to re-import {} ({e}) and could not preserve it as {}: {rename_err}",
                        segment.display(),
                        failed.display()
                    )
                })?;
                warn!(
                    segment = %failed.display(),
                    "failed to re-import rotated events segment; retained for a future startup re-import: {e}"
                );
            }
        }
    }

    Ok(imported)
}

/// Parse a single JSONL line and insert into decision_log.
///
/// Handles the hook's non-human decision records (itr#397): `auto_approved`
/// (decision approve), `deferred` (always-defer → decision ask), and `denied`
/// (fail-closed paths → decision deny). Each carries `decided_by` (the
/// layer/rule) and `config_hash` when the hook provided them.
pub async fn ingest_line(line: &str, state_db: &StateDb) -> anyhow::Result<Option<AuditDecision>> {
    let event: serde_json::Value = serde_json::from_str(line)?;

    let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
    let (decision, kind) = match event_type {
        "auto_approved" => ("approve", AuditDecisionKind::AutoApproved),
        "deferred" => ("ask", AuditDecisionKind::Deferred),
        "denied" => ("deny", AuditDecisionKind::Denied),
        _ => {
            debug!(event_type, "skipping non-decision event");
            return Ok(None);
        }
    };

    let agent_id = event
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let agent_type = event
        .get("agent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("claude_code");
    let project = event.get("project").and_then(|v| v.as_str()).unwrap_or("");
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
    let tool_use_id = event.get("tool_use_id").and_then(|v| v.as_str());
    let hook_event_name = event.get("hook_event_name").and_then(|v| v.as_str());
    let decided_by = event.get("decided_by").and_then(|v| v.as_str());
    let config_hash = event.get("config_hash").and_then(|v| v.as_str());
    let terminal_session_id = event
        .get("terminal_session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    // Serialize agent_type as JSON string to match existing format
    let agent_type_json = format!("\"{}\"", agent_type);

    let inserted = state_db
        .log_auto_approved(&crate::state::AutoApprovedEntry {
            agent_id,
            agent_type: &agent_type_json,
            project,
            tool_name,
            tool_input: &tool_input,
            timestamp,
            tool_use_id,
            hook_event_name,
            decision,
            decided_by,
            config_hash,
        })
        .await?;

    debug!(
        tool_name,
        agent_id, decision, "ingested hook decision event"
    );
    if !inserted {
        return Ok(None);
    }

    let ts = chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    // Forward the (already-redacted, see hook redact::redact_value + itr#89)
    // tool_input onto the wire ONLY for deferred native prompts, so the inbox
    // can render the literal AskUserQuestion / ExitPlanMode / Elicitation
    // question + options. Auto-approved/denied stay None to keep the wire lean.
    // A deferred event may carry `tool_input: null` (e.g. elicitations) — that
    // passes through gracefully as Some(Null) / None without crashing.
    let wire_tool_input = match kind {
        AuditDecisionKind::Deferred => event.get("tool_input").cloned(),
        AuditDecisionKind::AutoApproved | AuditDecisionKind::Denied => None,
    };

    Ok(Some(AuditDecision {
        kind,
        decided_by: decided_by.map(str::to_owned),
        project: PathBuf::from(project),
        agent_id: agent_id.to_owned(),
        terminal_session_id,
        tool_name: tool_name.to_owned(),
        ts,
        // Carry tool_use_id so a later `deferred_resolved` (itr#461) can match this
        // exact waiting row. A freshly-ingested deferral is by definition not yet
        // answered, so `resolved` stays None here (Some(true) is set only on the
        // reconnect snapshot for rows whose tool_result was later stamped).
        tool_use_id: tool_use_id.map(str::to_owned),
        resolved: None,
        tool_input: wire_tool_input,
    }))
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
        let audit = ingest_line(&line, &db)
            .await
            .unwrap()
            .expect("new event should produce audit decision");
        assert_eq!(audit.kind, AuditDecisionKind::AutoApproved);
        assert_eq!(audit.decided_by.as_deref(), None);
        assert_eq!(audit.tool_name, "Bash");
        // Auto-approved decisions keep the wire lean — no tool_input forwarded.
        assert_eq!(audit.tool_input, None);

        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool_name, "Bash");
        assert_eq!(history[0].agent_id, "cc-1");
    }

    #[tokio::test]
    async fn ingest_deferred_forwards_redacted_tool_input() {
        let db = test_db().await;
        // A deferred AskUserQuestion carries the (already-redacted) tool_input so
        // the inbox can render the literal question + options.
        let line = r#"{"event": "deferred", "agent_id": "cc-9", "tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Ship it?", "options": [{"label": "Yes"}]}]}, "timestamp": "2024-01-01T00:00:00Z", "tool_use_id": "def-1", "decided_by": "always_ask:intrinsic"}"#;
        let audit = ingest_line(line, &db)
            .await
            .unwrap()
            .expect("deferred event should produce audit decision");
        assert_eq!(audit.kind, AuditDecisionKind::Deferred);
        let input = audit
            .tool_input
            .expect("deferred decision must carry tool_input");
        assert_eq!(
            input["questions"][0]["question"],
            serde_json::json!("Ship it?")
        );
        assert_eq!(
            input["questions"][0]["options"][0]["label"],
            serde_json::json!("Yes")
        );
    }

    #[tokio::test]
    async fn ingest_deferred_null_tool_input_is_graceful() {
        let db = test_db().await;
        // Elicitations may arrive with tool_input: null — it passes through as
        // Some(Null) without crashing.
        let line = r#"{"event": "deferred", "agent_id": "cc-9", "tool_name": "Elicitation", "tool_input": null, "timestamp": "2024-01-01T00:00:00Z", "tool_use_id": "def-2", "decided_by": "always_ask:intrinsic"}"#;
        let audit = ingest_line(line, &db)
            .await
            .unwrap()
            .expect("deferred event should produce audit decision");
        assert_eq!(audit.kind, AuditDecisionKind::Deferred);
        assert_eq!(audit.tool_input, Some(serde_json::Value::Null));
    }

    #[tokio::test]
    async fn ingest_skips_non_auto_approved() {
        let db = test_db().await;
        let line = r#"{"event": "session_start", "agent_id": "cc-1"}"#;
        assert!(ingest_line(line, &db).await.unwrap().is_none());

        let history = db.query_history(None, 10).await.unwrap();
        assert!(
            history.is_empty(),
            "non-auto_approved events should be skipped"
        );
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
        let count = reimport_all(
            std::path::Path::new("/tmp/nonexistent_wisphive_test.jsonl"),
            &db,
        )
        .await
        .unwrap();
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
        assert_eq!(count, 1);

        // But only 1 row should be in the DB thanks to dedup
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(
            history.len(),
            1,
            "duplicate tool_use_id events should be deduplicated"
        );
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

        assert_eq!(
            after_second.len(),
            1,
            "reimport should be idempotent for events without tool_use_id"
        );
    }

    #[tokio::test]
    async fn reimport_rotated_segments_dedups_and_reaps_recovered_failed_segment() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let duplicate = auto_approved_event("Bash", "cc-1", Some("segment-1"));
        let recovered = auto_approved_event("Edit", "cc-1", Some("segment-2"));
        std::fs::write(
            log_dir.join("events-20260101-000000.jsonl"),
            format!("{duplicate}\n"),
        )
        .unwrap();
        let failed = log_dir.join("events-20260101-000001.failed.jsonl");
        std::fs::write(&failed, format!("{duplicate}\n{recovered}\n")).unwrap();

        let count = reimport_rotated_segments(&log_dir, &db).await.unwrap();
        assert_eq!(count, 2, "only new decision_log rows are counted");
        assert_eq!(db.query_history(None, 10).await.unwrap().len(), 2);
        assert!(
            !failed.exists(),
            "a successfully re-ingested failed segment is reaped"
        );

        assert_eq!(
            reimport_rotated_segments(&log_dir, &db).await.unwrap(),
            0,
            "a subsequent startup must not double-insert the normal segment"
        );
        assert_eq!(db.query_history(None, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn reimport_rotated_segments_skips_dangling_symlink() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let valid = auto_approved_event("Bash", "cc-1", Some("segment-1"));
        std::fs::write(
            log_dir.join("events-20260101-000000.jsonl"),
            format!("{valid}\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            log_dir.join("missing-segment.jsonl"),
            log_dir.join("events-20260101-000001.jsonl"),
        )
        .unwrap();

        assert_eq!(reimport_rotated_segments(&log_dir, &db).await.unwrap(), 1);
        assert_eq!(db.query_history(None, 10).await.unwrap().len(), 1);
    }

    // ════════════════════════════════════════════════════════════
    // rotation (#334)
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn rotate_events_log_is_non_lossy_and_resets() {
        let db = test_db().await;
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let content = format!(
            "{}\n{}\n",
            auto_approved_event("Bash", "cc-1", Some("t1")),
            auto_approved_event("Edit", "cc-1", Some("t2")),
        );
        std::fs::write(&events_path, &content).unwrap();

        let result = rotate_events_log(&events_path, &log_dir, &db).await;
        assert!(result.is_some(), "rotation should succeed");

        // Rotated segment landed in log_dir and the live file is now empty.
        let rotated: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("events-"))
            .collect();
        assert_eq!(rotated.len(), 1, "exactly one rotated segment expected");
        assert!(
            events_path.exists(),
            "fresh events.jsonl should be recreated"
        );
        assert_eq!(
            std::fs::metadata(&events_path).unwrap().len(),
            0,
            "fresh events.jsonl should be empty"
        );

        // No event was lost — both rows were ingested via the rotated segment.
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 2);
    }

    /// itr#301: events appended while the daemon was down must be ingested at
    /// startup — the tail used to seek straight to EOF and skip the backlog.
    #[tokio::test]
    async fn startup_ingests_backlog_written_while_daemon_was_down() {
        let db = Arc::new(test_db().await);
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        // Backlog written before the ingester exists (daemon down).
        let content = format!(
            "{}\n{}\n",
            auto_approved_event("Bash", "cc-1", Some("down-1")),
            auto_approved_event("Edit", "cc-1", Some("down-2")),
        );
        std::fs::write(&events_path, &content).unwrap();

        let (tui_tx, _) = broadcast::channel(16);
        let handle = spawn_event_ingest(events_path.clone(), log_dir, db.clone(), tui_tx);

        // The startup reimport is asynchronous — poll briefly.
        let mut history = Vec::new();
        for _ in 0..100 {
            history = db.query_history(None, 10).await.unwrap();
            if history.len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            history.len(),
            2,
            "backlog events must land in decision_log without manual reimport"
        );

        // A restart over the same file must not double-ingest (dedup).
        handle.abort();
        let count = reimport_all(&events_path, &db).await.unwrap();
        assert_eq!(
            count, 0,
            "reimport sees both lines but inserts no duplicates"
        );
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(history.len(), 2, "no duplicates after restart");
    }

    #[tokio::test]
    async fn file_identity_changes_when_file_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        std::fs::write(&path, b"a\n").unwrap();
        let id1 = file_identity(&path).await;
        assert!(id1.is_some());

        // Replace the file with a *larger* one (the case the shrink check misses).
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"much longer content than before\n").unwrap();
        let id2 = file_identity(&path).await;
        assert!(id2.is_some());
        assert_ne!(id1, id2, "replaced file should have a different inode");

        // Missing file → None.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(file_identity(&path).await, None);
    }

    #[tokio::test]
    async fn open_reader_at_clamps_offset_to_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        std::fs::write(&events_path, b"short\n").unwrap();

        // Offset past EOF (simulating a file that shrank) clamps to file length.
        let (_reader, offset) = open_reader_at(&events_path, 9999).await.unwrap();
        assert_eq!(offset, 6);
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

        let count = reimport_all(&events_path, &db).await.unwrap();
        assert_eq!(count, 1, "only inserted decision events are counted");

        // But only the auto_approved event should be in the DB
        let history = db.query_history(None, 10).await.unwrap();
        assert_eq!(
            history.len(),
            1,
            "only auto_approved events should be in the DB"
        );
        assert_eq!(history[0].tool_name, "Bash");
    }
}
