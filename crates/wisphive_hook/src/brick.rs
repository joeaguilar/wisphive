//! Brick detector (itr#538, ADR-0010).
//!
//! When the hook fail-closed denies repeatedly for the SAME config/perms
//! cause, the machine is "bricked by design" until the operator repairs the
//! state — but during that window the TUI/web may be unreachable or
//! unwatched, and the hook is the only component guaranteed to be running.
//! So the hook escalates passively: after [`THRESHOLD`] consecutive
//! same-cause denials it fires ONE OS notification (osascript / notify-send)
//! naming the broken file and the repair commands, and drops a
//! `~/.wisphive/BRICKED` marker file that `wisphive doctor` and the daemon
//! can surface.
//!
//! Invariants:
//! - The detector NEVER changes the hook's decision and never adds a failure
//!   path: every filesystem write and every notification is best-effort.
//!   Fail-closed stays fail-closed (ADR-0010); this module only shortens the
//!   time until a human learns why.
//! - Notifications are rate-limited to once per cause per hour. A different
//!   cause fires again immediately; a healthy invocation clears all detector
//!   state, so a repair followed by a fresh break re-notifies.
//! - NO repairs. Repair is a deliberate out-of-band act (`wisphive doctor
//!   --fix-perms`, `scripts/wisphive-rescue.sh`) per ADR-0010/itr#534.
//!
//! Persistence choice (documented per itr#538): the rate-limit state is
//! written primarily under the state dir itself
//! (`~/.wisphive/brick-state.json`) so it is visible next to the marker and
//! survives reboots — but the state dir may be the very thing that is broken
//! (foreign-owned, unwritable), so a fallback copy lives in the system temp
//! dir keyed by euid (`$TMPDIR/wisphive-brick-<uid>.json`, mode 0600,
//! O_NOFOLLOW, ownership-checked before trust). Worst case for a hostile or
//! lost tmp file is a duplicate or suppressed *informational* notification —
//! never a gating change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Consecutive same-cause denials before the single escalation fires.
pub const THRESHOLD: u32 = 3;

/// Minimum seconds between notifications for the same cause.
pub const RENOTIFY_SECS: u64 = 3600;

/// Marker file name under the state dir (best-effort, perms-tolerant).
pub const MARKER_NAME: &str = "BRICKED";

/// Rate-limit state file name under the state dir.
pub const STATE_NAME: &str = "brick-state.json";

/// Cap on tracked causes in the rate-limit map (drops oldest beyond this).
const MAX_TRACKED_CAUSES: usize = 8;

/// Cap on a state file we are willing to parse.
const STATE_MAX_BYTES: u64 = 64 * 1024;

/// Test/e2e seam: when this env var names a file, the notification is
/// appended there (one line per notification) instead of reaching the OS
/// notifier. Notifications are informational only, so redirecting them is
/// not a gating bypass; it lets tests and the red-team script count
/// notifications without spamming the operator's desktop.
pub const NOTIFY_CAPTURE_ENV: &str = "WISPHIVE_BRICK_NOTIFY_CAPTURE";

/// One passive escalation.
pub struct BrickNotification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct DetectorState {
    /// Cause hash of the current consecutive-denial run.
    cause: String,
    /// Length of the current same-cause run.
    consecutive: u32,
    /// cause hash -> epoch seconds of its last notification.
    notified: BTreeMap<String, u64>,
}

/// Record a config/perms fail-closed denial (production entry point).
/// Best-effort: never panics, never errors, never changes the decision.
pub fn record_denial(wisphive_dir: &Path, cause_message: &str, full_deny_message: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    process_denial(
        &candidate_state_paths(wisphive_dir),
        wisphive_dir,
        cause_message,
        full_deny_message,
        now,
        &mut send_os_notification,
    );
}

/// Clear marker + rate-limit state after a healthy mode read (production
/// entry point). Best-effort; a later break — even the same cause — then
/// notifies afresh.
pub fn clear_on_healthy(wisphive_dir: &Path) {
    let _ = std::fs::remove_file(wisphive_dir.join(MARKER_NAME));
    for path in candidate_state_paths(wisphive_dir) {
        let _ = std::fs::remove_file(path);
    }
}

/// Where detector state may live, in preference order. See module docs.
fn candidate_state_paths(wisphive_dir: &Path) -> Vec<PathBuf> {
    let uid = effective_uid();
    vec![
        wisphive_dir.join(STATE_NAME),
        std::env::temp_dir().join(format!("wisphive-brick-{uid}.json")),
    ]
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    0
}

/// Core detector logic, injectable for tests (paths, clock, notifier).
fn process_denial(
    state_paths: &[PathBuf],
    wisphive_dir: &Path,
    cause_message: &str,
    full_deny_message: &str,
    now: u64,
    notify: &mut dyn FnMut(&BrickNotification),
) {
    let cause = cause_hash(cause_message);
    let mut state = load_state(state_paths).unwrap_or_default();
    if state.cause != cause {
        state.cause = cause.clone();
        state.consecutive = 0;
    }
    state.consecutive = state.consecutive.saturating_add(1);

    let rate_limited = state
        .notified
        .get(&cause)
        .is_some_and(|&at| now.saturating_sub(at) < RENOTIFY_SECS);
    if state.consecutive >= THRESHOLD && !rate_limited {
        state.notified.insert(cause.clone(), now);
        while state.notified.len() > MAX_TRACKED_CAUSES {
            // Drop the least-recently-notified cause.
            if let Some(oldest) = state
                .notified
                .iter()
                .min_by_key(|&(_, &at)| at)
                .map(|(key, _)| key.clone())
            {
                state.notified.remove(&oldest);
            } else {
                break;
            }
        }
        write_marker(wisphive_dir, cause_message, full_deny_message, now);
        notify(&build_notification(cause_message));
    }
    store_state(state_paths, &state);
}

/// Stable identity for "the SAME cause": truncated SHA-256 of the validator
/// error text (which embeds the failing path and the observed mode/uid, so a
/// repair or a different broken file yields a different cause).
fn cause_hash(cause_message: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(cause_message.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn build_notification(cause_message: &str) -> BrickNotification {
    let first_line = cause_message.lines().next().unwrap_or(cause_message);
    BrickNotification {
        title: "Wisphive: gating bricked (fail-closed)".to_string(),
        body: format!(
            "{} — run 'wisphive doctor --fix-perms' or 'scripts/wisphive-rescue.sh'",
            sanitize_for_notification(first_line)
        ),
    }
}

/// Remove control characters before untrusted text reaches the platform
/// notification renderer (mirrors the daemon's `sanitize_for_log`).
fn sanitize_for_notification(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

// ── state persistence (hand-rolled JSON over serde_json::Value) ─────────────

fn state_to_json(state: &DetectorState) -> serde_json::Value {
    serde_json::json!({
        "cause": state.cause,
        "consecutive": state.consecutive,
        "notified": state.notified,
    })
}

fn state_from_json(value: &serde_json::Value) -> Option<DetectorState> {
    let cause = value.get("cause")?.as_str()?.to_string();
    let consecutive = u32::try_from(value.get("consecutive")?.as_u64()?).ok()?;
    let mut notified = BTreeMap::new();
    if let Some(map) = value.get("notified").and_then(|v| v.as_object()) {
        for (key, at) in map {
            notified.insert(key.clone(), at.as_u64()?);
        }
    }
    Some(DetectorState {
        cause,
        consecutive,
        notified,
    })
}

/// First candidate that holds a parseable, owner-trusted state file wins.
fn load_state(state_paths: &[PathBuf]) -> Option<DetectorState> {
    for path in state_paths {
        if let Some(contents) = read_owned_file(path)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents)
            && let Some(state) = state_from_json(&value)
        {
            return Some(state);
        }
    }
    None
}

/// Persist to every writable candidate (so the tmp fallback stays current
/// even when the state dir happens to be writable). All best-effort.
fn store_state(state_paths: &[PathBuf], state: &DetectorState) {
    let serialized = state_to_json(state).to_string();
    for path in state_paths {
        let _ = write_private_file(path, serialized.as_bytes());
    }
}

/// Read a file only if it is a non-symlink regular file owned by the
/// effective uid and small enough to parse. The tmp fallback lives in a
/// world-writable dir; never trust foreign or symlinked content.
#[cfg(unix)]
fn read_owned_file(path: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.len() > STATE_MAX_BYTES
    {
        return None;
    }
    let mut contents = String::new();
    file.take(STATE_MAX_BYTES)
        .read_to_string(&mut contents)
        .ok()?;
    Some(contents)
}

#[cfg(not(unix))]
fn read_owned_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Create/overwrite `path` with mode 0600 without following symlinks.
/// Write-then-rename via an owner-exclusive sibling so a partially written
/// state file is never visible at `path`.
#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let pid = std::process::id();
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp.{pid}"));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Drop the `BRICKED` marker next to the broken state (best-effort,
/// perms-tolerant: an unwritable state dir just skips the marker — the
/// notification already fired).
fn write_marker(wisphive_dir: &Path, cause_message: &str, full_deny_message: &str, now: u64) {
    let contents = format!(
        "Wisphive brick detector (itr#538, ADR-0010)\n\
         epoch: {now}\n\
         cause: {cause_message}\n\n\
         Every hook event is being DENIED (fail-closed) until this is repaired.\n\
         Full denial message:\n{full_deny_message}\n\n\
         This marker is cleared automatically on the next healthy hook invocation.\n"
    );
    let _ = write_private_file(&wisphive_dir.join(MARKER_NAME), contents.as_bytes());
}

// ── OS notification (osascript / notify-send), capture seam for tests ──────

fn send_os_notification(notification: &BrickNotification) {
    if let Ok(capture) = std::env::var(NOTIFY_CAPTURE_ENV)
        && !capture.is_empty()
    {
        append_capture_line(Path::new(&capture), notification);
        return;
    }
    spawn_platform_notifier(notification);
}

fn append_capture_line(path: &Path, notification: &BrickNotification) {
    use std::io::Write;
    let line = format!(
        "{}\t{}\n",
        notification.title.replace(['\t', '\n'], " "),
        notification.body.replace(['\t', '\n'], " ")
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(target_os = "macos")]
fn spawn_platform_notifier(notification: &BrickNotification) {
    // Same injection boundary as the daemon's notifier: the AppleScript
    // string literal only needs backslash and double-quote escaping, and
    // control characters were already stripped by the sanitizer.
    fn escape(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape(&notification.body),
        escape(&notification.title)
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_platform_notifier(notification: &BrickNotification) {
    let _ = std::process::Command::new("notify-send")
        .arg(&notification.title)
        .arg(&notification.body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(not(unix))]
fn spawn_platform_notifier(_notification: &BrickNotification) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &Path) -> Vec<PathBuf> {
        vec![dir.join(STATE_NAME)]
    }

    fn deny_n(n: u32, state_paths: &[PathBuf], dir: &Path, cause: &str, now: u64, count: &mut u32) {
        for _ in 0..n {
            let mut notify = |_: &BrickNotification| *count += 1;
            process_denial(
                state_paths,
                dir,
                cause,
                "full deny message",
                now,
                &mut notify,
            );
        }
    }

    #[test]
    fn fifty_rapid_denials_fire_exactly_one_notification_and_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let mut count = 0;
        deny_n(50, &paths, dir.path(), "mode file 0644", 1000, &mut count);
        assert_eq!(count, 1, "50 rapid same-cause denials must fire exactly 1");
        let marker = std::fs::read_to_string(dir.path().join(MARKER_NAME)).unwrap();
        assert!(marker.contains("mode file 0644"));
        assert!(marker.contains("full deny message"));
    }

    #[test]
    fn below_threshold_stays_silent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let mut count = 0;
        deny_n(THRESHOLD - 1, &paths, dir.path(), "cause", 1000, &mut count);
        assert_eq!(count, 0);
        assert!(!dir.path().join(MARKER_NAME).exists());
    }

    #[test]
    fn different_cause_fires_again_but_repeated_old_cause_is_rate_limited() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let mut count = 0;
        deny_n(5, &paths, dir.path(), "cause A", 1000, &mut count);
        assert_eq!(count, 1);
        deny_n(5, &paths, dir.path(), "cause B", 1010, &mut count);
        assert_eq!(count, 2, "a different cause must fire again immediately");
        // Back to cause A within the hour: consecutive run restarts and the
        // per-cause rate limit suppresses a duplicate.
        deny_n(5, &paths, dir.path(), "cause A", 1020, &mut count);
        assert_eq!(count, 2, "same cause within an hour must not refire");
    }

    #[test]
    fn same_cause_renotifies_after_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let mut count = 0;
        deny_n(5, &paths, dir.path(), "cause", 1000, &mut count);
        assert_eq!(count, 1);
        deny_n(
            1,
            &paths,
            dir.path(),
            "cause",
            1000 + RENOTIFY_SECS,
            &mut count,
        );
        assert_eq!(count, 2, "an hour later the same cause must renotify");
    }

    #[test]
    fn healthy_invocation_clears_marker_and_state_so_a_new_break_refires() {
        let dir = tempfile::tempdir().unwrap();
        let paths = candidate_state_paths(dir.path());
        let mut count = 0;
        deny_n(5, &paths, dir.path(), "cause", 1000, &mut count);
        assert_eq!(count, 1);
        assert!(dir.path().join(MARKER_NAME).exists());

        clear_on_healthy(dir.path());
        assert!(!dir.path().join(MARKER_NAME).exists());
        assert!(!dir.path().join(STATE_NAME).exists());

        // Re-break with the SAME cause: fresh state means a fresh escalation.
        deny_n(5, &paths, dir.path(), "cause", 1100, &mut count);
        assert_eq!(count, 2);
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_state_dir_falls_back_to_second_path_and_still_fires_once() {
        use std::os::unix::fs::PermissionsExt;
        let broken = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        std::fs::set_permissions(broken.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let paths = vec![
            broken.path().join(STATE_NAME),
            fallback.path().join("brick.json"),
        ];
        let mut count = 0;
        deny_n(50, &paths, broken.path(), "cause", 1000, &mut count);
        assert_eq!(count, 1, "rate limiting must survive via the tmp fallback");
        assert!(fallback.path().join("brick.json").exists());
        // Marker write into the unwritable dir was skipped, not fatal.
        assert!(!broken.path().join(MARKER_NAME).exists());
        std::fs::set_permissions(broken.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_or_foreign_state_file_is_ignored_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.json");
        std::fs::write(&target, "{\"cause\":\"x\",\"consecutive\":99}").unwrap();
        let link = dir.path().join(STATE_NAME);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(load_state(&[link]).is_none(), "symlink must not be trusted");
    }

    #[test]
    fn state_roundtrips_through_json() {
        let mut state = DetectorState {
            cause: "abc".into(),
            consecutive: 7,
            notified: BTreeMap::new(),
        };
        state.notified.insert("abc".into(), 42);
        let parsed = state_from_json(&state_to_json(&state)).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn cause_hash_distinguishes_messages() {
        assert_ne!(cause_hash("dir 0755"), cause_hash("mode 0644"));
        assert_eq!(cause_hash("dir 0755"), cause_hash("dir 0755"));
    }

    #[test]
    fn notification_names_repair_commands_and_strips_control_chars() {
        let n = build_notification("mode file \u{1b}[31mbad\u{0}");
        assert!(n.body.contains("wisphive doctor --fix-perms"));
        assert!(n.body.contains("wisphive-rescue.sh"));
        assert!(!n.body.chars().any(|c| c.is_control()));
    }
}
