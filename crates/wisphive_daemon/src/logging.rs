//! Daemon logging: rolling JSON file + in-memory ring buffer for live tailing.
//!
//! Composes a tracing subscriber with three sinks:
//! 1. stderr (default WARN) — keeps the foreground terminal quiet.
//! 2. JSON file under `~/.wisphive/logs/wisphive.log.YYYY-MM-DD` via
//!    `tracing-appender`'s daily roller, wrapped in a non-blocking writer.
//! 3. [`StoreLayer`] — pushes structured records into a ring buffer with a
//!    broadcast fanout so future web/TUI views can stream the daemon's logs.
//!
//! The file appender's [`tracing_appender::non_blocking::WorkerGuard`] is held
//! by [`LogGuards`]; callers must keep it alive for the lifetime of the
//! daemon and explicitly drop it before any `std::process::exit` so buffered
//! records are flushed to disk.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::Level;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// Per-subscriber pending capacity for the [`LogStore`] broadcast channel.
/// Sized to match the default ring buffer capacity so a subscriber that's
/// briefly unscheduled can still catch up to the full retained window
/// before it starts seeing `RecvError::Lagged`.
const STORE_BROADCAST_CAPACITY: usize = 4096;

/// A single log record captured by [`StoreLayer`]. Serializable so the web
/// bridge can ship it to the browser as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    /// RFC3339 timestamp (UTC).
    pub ts: String,
    /// Level name: `ERROR` / `WARN` / `INFO` / `DEBUG` / `TRACE`.
    pub level: String,
    /// Tracing target (typically `module::path`).
    pub target: String,
    /// Formatted `message` field.
    pub message: String,
    /// Remaining structured fields as a JSON object.
    pub fields: serde_json::Value,
}

/// Numeric severity used for `tail` filtering. Higher = more severe.
fn severity(name: &str) -> u8 {
    match name {
        "ERROR" => 5,
        "WARN" => 4,
        "INFO" => 3,
        "DEBUG" => 2,
        "TRACE" => 1,
        _ => 0,
    }
}

fn level_severity(level: Level) -> u8 {
    severity(level.as_str())
}

/// Bounded ring buffer of [`LogRecord`]s with a broadcast fanout.
pub struct LogStore {
    capacity: usize,
    inner: Mutex<VecDeque<LogRecord>>,
    tx: broadcast::Sender<LogRecord>,
}

impl LogStore {
    /// Create a new store with the given capacity. Capacity is clamped to at
    /// least 1 so `push` always has somewhere to land.
    pub fn new(capacity: usize) -> Arc<Self> {
        let capacity = capacity.max(1);
        let (tx, _) = broadcast::channel(STORE_BROADCAST_CAPACITY);
        Arc::new(Self {
            capacity,
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            tx,
        })
    }

    /// Lock the inner deque, recovering from poison so a panic in one tracing
    /// callsite never cascades into killing the daemon on the next log call.
    /// The contained data is just `LogRecord`s — there's no invariant that
    /// can be left "half-broken" by a panic mid-mutation.
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, VecDeque<LogRecord>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append a record, evicting the oldest entry if the buffer is full, then
    /// fan it out to live subscribers. A send failure (no receivers) is
    /// expected and silently ignored.
    pub fn push(&self, record: LogRecord) {
        {
            let mut guard = self.lock_inner();
            if guard.len() >= self.capacity {
                guard.pop_front();
            }
            guard.push_back(record.clone());
        }
        let _ = self.tx.send(record);
    }

    /// Return up to the last `n` records whose level is at least `min_level`.
    pub fn tail(&self, n: usize, min_level: Level) -> Vec<LogRecord> {
        let min = level_severity(min_level);
        let guard = self.lock_inner();
        let filtered: Vec<LogRecord> = guard
            .iter()
            .filter(|r| severity(&r.level) >= min)
            .cloned()
            .collect();
        let len = filtered.len();
        let start = len.saturating_sub(n);
        filtered.into_iter().skip(start).collect()
    }

    /// Subscribe to new records.
    ///
    /// Backpressure: the channel is bounded at [`STORE_BROADCAST_CAPACITY`]
    /// pending records *per subscriber*. Slow consumers will receive
    /// `Err(RecvError::Lagged(n))` once they fall further behind than that;
    /// callers MUST treat that as recoverable (skip and call `recv` again),
    /// not as a terminal stream error.
    pub fn subscribe(&self) -> broadcast::Receiver<LogRecord> {
        self.tx.subscribe()
    }

    /// Current number of buffered records (for tests and diagnostics).
    pub fn len(&self) -> usize {
        self.lock_inner().len()
    }

    /// `true` if the buffer is currently empty.
    pub fn is_empty(&self) -> bool {
        self.lock_inner().is_empty()
    }
}

/// Visit a tracing event's fields into a JSON object plus a flat `message`.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let s = format!("{:?}", value);
        if field.name() == "message" {
            self.message = s;
        } else {
            self.fields
                .insert(field.name().to_string(), serde_json::Value::String(s));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        let v = match serde_json::Number::from_f64(value) {
            Some(num) => serde_json::Value::Number(num),
            // JSON has no NaN/Inf encoding; preserve the field with a string
            // marker so a debugging session sees "the value was non-finite"
            // rather than a silently missing field.
            None => serde_json::Value::String(value.to_string()),
        };
        self.fields.insert(field.name().to_string(), v);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
}

/// Tracing layer that pushes each event into a [`LogStore`].
pub struct StoreLayer {
    store: Arc<LogStore>,
}

impl StoreLayer {
    pub fn new(store: Arc<LogStore>) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for StoreLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        let record = LogRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message: visitor.message,
            fields: serde_json::Value::Object(visitor.fields),
        };
        self.store.push(record);
    }
}

/// Drop guards returned by [`init`]. Owning this keeps the non-blocking file
/// writer's worker thread alive; dropping it flushes pending writes.
pub struct LogGuards {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Compose and install the daemon's tracing subscriber. Returns guards the
/// caller MUST keep alive for the daemon lifetime, and explicitly drop before
/// `std::process::exit` to flush buffered file writes.
///
/// Layout:
/// - Global [`EnvFilter`] from `RUST_LOG`, defaulting to `info`.
/// - stderr fmt layer additionally clamped to `stderr_level` (default WARN).
/// - JSON fmt layer to a daily-rolled file under `log_dir`.
/// - [`StoreLayer`] that pushes records into `store`.
pub fn init(
    log_dir: &Path,
    store: Arc<LogStore>,
    stderr_level: Level,
) -> anyhow::Result<LogGuards> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::from_level(stderr_level));

    // File logging is intentionally best-effort: `tracing-appender`'s
    // non_blocking writer drops records when its bounded channel fills, and
    // the worker thread can die without surfacing an error to us. The ring
    // buffer + stderr layer are the durable signals; the file is for
    // post-mortem forensics.
    let file_appender = tracing_appender::rolling::daily(log_dir, "wisphive.log");
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_writer);

    let store_layer = StoreLayer::new(store);

    Registry::default()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .with(store_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to install tracing subscriber: {e}"))?;

    Ok(LogGuards {
        _file_guard: file_guard,
    })
}

/// Whether the pruner is allowed to delete a file with this name by age.
///
/// This is an explicit **allowlist**, not a denylist: an unrecognized file is
/// kept. The cost of accidentally retaining a log file is disk; the cost of
/// accidentally deleting a data file is permanent loss. `prune_old_files`
/// previously reaped *every* file in `log_dir`, which silently deleted the
/// decision archive and `*.failed.jsonl` recovery segments (see #339).
///
/// Reapable:
/// - `wisphive.log*` — rolling daemon logs (mirrored to the in-memory
///   `LogStore`; pure operational telemetry, safe to drop).
/// - `events-*.jsonl` rotated hook-event segments — re-ingested into
///   `decision_log` before rotation, so the on-disk copy is redundant.
///
/// Never reaped:
/// - `*.failed.jsonl` — event segments whose re-ingest FAILED; the only copy of
///   that telemetry until an operator (or the startup sweep, #336) re-imports it.
/// - `decision_log.jsonl*` — the decision archive sink and its rotated segments
///   (long-term audit data; durable-path policy tracked in #340).
fn is_reapable(name: &str) -> bool {
    if name.ends_with(".failed.jsonl") {
        return false;
    }
    if name.starts_with("decision_log.jsonl") {
        return false;
    }
    name.starts_with("wisphive.log") || (name.starts_with("events-") && name.ends_with(".jsonl"))
}

/// Delete reapable files (see [`is_reapable`]) in `log_dir` whose mtime is older
/// than `retention_days`. Best effort — individual unlink failures are swallowed
/// so a permission glitch on one file can't abort daemon startup.
pub fn prune_old_files(log_dir: &Path, retention_days: u64) -> std::io::Result<()> {
    let cutoff = match SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days.saturating_mul(86_400)))
    {
        Some(t) => t,
        None => return Ok(()),
    };

    let entries = match std::fs::read_dir(log_dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        // Only reap recognized, redundant log/segment files. Unknown files and
        // data files (decision archive, `.failed.jsonl` recovery) are kept
        // regardless of age — see `is_reapable`.
        match entry.file_name().to_str() {
            Some(name) if is_reapable(name) => {}
            _ => continue,
        }
        if modified < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn record(level: &str, msg: &str) -> LogRecord {
        LogRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            level: level.to_string(),
            target: "test".to_string(),
            message: msg.to_string(),
            fields: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn ring_buffer_evicts_at_capacity() {
        let store = LogStore::new(3);
        store.push(record("INFO", "a"));
        store.push(record("INFO", "b"));
        store.push(record("INFO", "c"));
        store.push(record("INFO", "d"));

        let tail = store.tail(10, Level::TRACE);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].message, "b");
        assert_eq!(tail[1].message, "c");
        assert_eq!(tail[2].message, "d");
    }

    #[test]
    fn tail_respects_level_filter() {
        let store = LogStore::new(16);
        store.push(record("DEBUG", "d1"));
        store.push(record("INFO", "i1"));
        store.push(record("WARN", "w1"));
        store.push(record("ERROR", "e1"));
        store.push(record("INFO", "i2"));

        let warn_plus = store.tail(10, Level::WARN);
        assert_eq!(warn_plus.len(), 2);
        assert_eq!(warn_plus[0].message, "w1");
        assert_eq!(warn_plus[1].message, "e1");

        let info_plus = store.tail(10, Level::INFO);
        assert_eq!(info_plus.len(), 4);

        let only_last_two = store.tail(2, Level::TRACE);
        assert_eq!(only_last_two.len(), 2);
        assert_eq!(only_last_two[0].message, "e1");
        assert_eq!(only_last_two[1].message, "i2");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_receives_pushed_record() {
        let store = LogStore::new(8);
        let mut rx = store.subscribe();

        let store_for_push = store.clone();
        let pusher = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            store_for_push.push(record("INFO", "live"));
        });

        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("subscribe channel timed out")
            .expect("subscribe channel closed");
        assert_eq!(received.message, "live");
        pusher.await.unwrap();
    }

    #[test]
    fn pruner_deletes_files_older_than_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("wisphive.log.2020-01-01");
        std::fs::write(&stale, b"stale").unwrap();

        // Make sure the file's mtime is strictly in the past relative to the
        // cutoff we'll compute next.
        std::thread::sleep(Duration::from_millis(20));

        // retention_days = 0 -> cutoff is `now`; the file written above is
        // older, so it should be pruned.
        prune_old_files(dir.path(), 0).unwrap();
        assert!(!stale.exists(), "file older than cutoff should be pruned");
    }

    #[test]
    fn pruner_retains_files_within_retention() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("wisphive.log.today");
        std::fs::write(&fresh, b"fresh").unwrap();

        // retention_days = 365 -> cutoff is a year ago; the fresh file
        // should be retained.
        prune_old_files(dir.path(), 365).unwrap();
        assert!(fresh.exists(), "fresh file should be retained");
    }

    #[test]
    fn store_layer_captures_event_via_subscriber() {
        // Locks in that `StoreLayer::on_event` + `FieldVisitor` actually wire
        // up correctly when fed a real `tracing::Event` (rather than the
        // synthetic `LogRecord`s the other tests use).
        let store = LogStore::new(8);
        let subscriber = tracing_subscriber::registry().with(StoreLayer::new(store.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "test_target", count = 3u64, name = "alice", "hello");
        });

        let records = store.tail(10, Level::TRACE);
        assert_eq!(
            records.len(),
            1,
            "exactly one event should have been captured"
        );
        let r = &records[0];
        assert_eq!(r.level, "INFO");
        assert_eq!(r.target, "test_target");
        assert_eq!(r.message, "hello");
        assert_eq!(r.fields["count"], serde_json::json!(3));
        assert_eq!(r.fields["name"], serde_json::json!("alice"));
    }

    #[test]
    fn record_f64_preserves_nan_as_string_marker() {
        let store = LogStore::new(4);
        let subscriber = tracing_subscriber::registry().with(StoreLayer::new(store.clone()));

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "test_target", ratio = f64::NAN, "trouble");
        });

        let records = store.tail(10, Level::TRACE);
        assert_eq!(records.len(), 1);
        let ratio = &records[0].fields["ratio"];
        assert!(
            ratio.is_string(),
            "non-finite f64 should be preserved as a string marker, got {ratio:?}"
        );
    }

    #[test]
    fn pruner_only_reaps_allowlisted_files() {
        let dir = tempfile::tempdir().unwrap();

        // Reapable: daemon logs + re-ingested rotated event segments.
        let daemon_log = dir.path().join("wisphive.log.2020-01-01");
        let event_seg = dir.path().join("events-20200101-000000.jsonl");
        // Protected: failed-reimport recovery + the decision archive (live and
        // rotated). These hold the only copy of audit/telemetry data.
        let failed_seg = dir.path().join("events-20200101-000000.failed.jsonl");
        let archive = dir.path().join("decision_log.jsonl");
        let archive_rotated = dir.path().join("decision_log.jsonl.20200101-000000");
        // Unknown file: kept (allowlist, not denylist).
        let unknown = dir.path().join("important-backup.db");

        for f in [
            &daemon_log,
            &event_seg,
            &failed_seg,
            &archive,
            &archive_rotated,
            &unknown,
        ] {
            std::fs::write(f, b"x").unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));

        // retention_days = 0 -> everything is "older than cutoff" by age.
        prune_old_files(dir.path(), 0).unwrap();

        assert!(!daemon_log.exists(), "daemon log should be reaped");
        assert!(
            !event_seg.exists(),
            "rotated event segment should be reaped"
        );
        assert!(
            failed_seg.exists(),
            ".failed.jsonl recovery segment must never be reaped"
        );
        assert!(
            archive.exists(),
            "decision archive sink must never be reaped"
        );
        assert!(
            archive_rotated.exists(),
            "rotated decision archive must never be reaped"
        );
        assert!(unknown.exists(), "unknown files must not be reaped");
    }

    #[test]
    fn pruner_handles_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        prune_old_files(&missing, 14).unwrap();
    }
}
