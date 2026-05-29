//! Non-destructive resource alerts.
//!
//! Wisphive's audit archive is never auto-deleted (itr#340): losing decision
//! history defeats the point of the control plane. Instead, when the archive
//! grows large or the host is low on disk, the daemon raises an alert that
//! surfaces as a TUI/web banner and a `warn!` log — prompting the operator to
//! act (move/compress the archive, free disk) rather than silently reaping data.

use std::path::Path;
use wisphive_protocol::DiskAlertKind;

/// A measured snapshot of the resources the alerter watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    /// Total bytes of the on-disk decision archive (`decision_log.jsonl` plus
    /// its rotated `decision_log.jsonl.<ts>` siblings).
    pub archive_bytes: u64,
    /// Free bytes available to an unprivileged user on the state filesystem.
    pub free_bytes: u64,
}

/// Thresholds for the two watched conditions; `0` disables that check.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub archive_max_bytes: u64,
    pub disk_free_min_bytes: u64,
}

/// Latch so each condition alerts once per crossing, not every tick. A raise is
/// emitted on the false→true edge and a clear on the true→false edge.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlertState {
    archive_active: bool,
    disk_active: bool,
}

/// An alert transition to surface to clients and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertEvent {
    pub kind: DiskAlertKind,
    /// `true` = condition entered (raise); `false` = condition cleared.
    pub active: bool,
    pub message: String,
}

/// Sum the size of the decision archive sink and its rotated segments in
/// `log_dir`. Best effort: unreadable dir/entries contribute 0.
pub fn archive_bytes(log_dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let is_archive = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("decision_log.jsonl"));
        if !is_archive {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Free bytes available on the filesystem containing `path`. `None` if the
/// query fails (caller treats unknown as "plenty" so a failed probe never
/// fires a false low-disk alert).
pub fn free_bytes(path: &Path) -> Option<u64> {
    free_bytes_impl(path)
}

#[cfg(unix)]
fn free_bytes_impl(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `statvfs` is a valid uninitialized state; we pass a
    // valid NUL-terminated path and only read fields after a 0 return code.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    // f_bavail: blocks available to unprivileged users; f_frsize: fragment size.
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(not(unix))]
fn free_bytes_impl(_path: &Path) -> Option<u64> {
    None
}

/// Compare `usage` against `thresholds`, update the latch `state`, and return
/// the transitions to surface this tick (raises and clears). Pure — no IO — so
/// the edge logic is unit-testable without touching a real filesystem.
pub fn evaluate(usage: DiskUsage, thresholds: Thresholds, state: &mut AlertState) -> Vec<AlertEvent> {
    let mut events = Vec::new();

    if thresholds.archive_max_bytes > 0 {
        let over = usage.archive_bytes > thresholds.archive_max_bytes;
        if over && !state.archive_active {
            state.archive_active = true;
            events.push(AlertEvent {
                kind: DiskAlertKind::ArchiveSize,
                active: true,
                message: format!(
                    "Audit archive is {} (over the {} alert threshold). Wisphive never \
                     deletes audit data — move or compress ~/.wisphive/logs/decision_log.jsonl* \
                     to reclaim space.",
                    human_bytes(usage.archive_bytes),
                    human_bytes(thresholds.archive_max_bytes),
                ),
            });
        } else if !over && state.archive_active {
            state.archive_active = false;
            events.push(AlertEvent {
                kind: DiskAlertKind::ArchiveSize,
                active: false,
                message: format!(
                    "Audit archive back under threshold ({}).",
                    human_bytes(usage.archive_bytes)
                ),
            });
        }
    }

    if thresholds.disk_free_min_bytes > 0 {
        let low = usage.free_bytes < thresholds.disk_free_min_bytes;
        if low && !state.disk_active {
            state.disk_active = true;
            events.push(AlertEvent {
                kind: DiskAlertKind::LowDiskSpace,
                active: true,
                message: format!(
                    "Low disk: {} free on the Wisphive state filesystem (under the {} floor).",
                    human_bytes(usage.free_bytes),
                    human_bytes(thresholds.disk_free_min_bytes),
                ),
            });
        } else if !low && state.disk_active {
            state.disk_active = false;
            events.push(AlertEvent {
                kind: DiskAlertKind::LowDiskSpace,
                active: false,
                message: format!("Free disk recovered ({} free).", human_bytes(usage.free_bytes)),
            });
        }
    }

    events
}

/// Measure resources, evaluate against `thresholds`, log any transition, and
/// return the events for the caller to broadcast to clients. Unknown free space
/// is treated as `u64::MAX` (plenty) so a failed `statvfs` never false-alarms.
pub fn check(
    log_dir: &Path,
    state_fs_path: &Path,
    thresholds: Thresholds,
    state: &mut AlertState,
) -> Vec<AlertEvent> {
    let usage = DiskUsage {
        archive_bytes: archive_bytes(log_dir),
        free_bytes: free_bytes(state_fs_path).unwrap_or(u64::MAX),
    };
    let events = evaluate(usage, thresholds, state);
    for ev in &events {
        if ev.active {
            tracing::warn!(kind = ?ev.kind, "{}", ev.message);
        } else {
            tracing::info!(kind = ?ev.kind, "{}", ev.message);
        }
    }
    events
}

/// Render a byte count as a human-readable string (e.g. `10.0 GiB`).
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn thresholds() -> Thresholds {
        Thresholds {
            archive_max_bytes: 10 * GIB,
            disk_free_min_bytes: 10 * GIB,
        }
    }

    #[test]
    fn raises_once_then_stays_latched() {
        let mut state = AlertState::default();
        let over = DiskUsage {
            archive_bytes: 11 * GIB,
            free_bytes: 100 * GIB,
        };

        let first = evaluate(over, thresholds(), &mut state);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, DiskAlertKind::ArchiveSize);
        assert!(first[0].active);

        // Still over on the next tick → no duplicate raise.
        let second = evaluate(over, thresholds(), &mut state);
        assert!(second.is_empty(), "latched condition must not re-raise");
    }

    #[test]
    fn clears_when_back_under_threshold() {
        let mut state = AlertState::default();
        let over = DiskUsage {
            archive_bytes: 11 * GIB,
            free_bytes: 100 * GIB,
        };
        let under = DiskUsage {
            archive_bytes: GIB,
            free_bytes: 100 * GIB,
        };

        evaluate(over, thresholds(), &mut state);
        let cleared = evaluate(under, thresholds(), &mut state);
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].kind, DiskAlertKind::ArchiveSize);
        assert!(!cleared[0].active, "dropping under threshold should clear");
    }

    #[test]
    fn low_disk_and_archive_are_independent() {
        let mut state = AlertState::default();
        let usage = DiskUsage {
            archive_bytes: 11 * GIB, // over
            free_bytes: 2 * GIB,     // low
        };
        let events = evaluate(usage, thresholds(), &mut state);
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e.kind == DiskAlertKind::ArchiveSize && e.active));
        assert!(events.iter().any(|e| e.kind == DiskAlertKind::LowDiskSpace && e.active));
    }

    #[test]
    fn zero_threshold_disables_check() {
        let mut state = AlertState::default();
        let usage = DiskUsage {
            archive_bytes: 999 * GIB,
            free_bytes: 0,
        };
        let disabled = Thresholds {
            archive_max_bytes: 0,
            disk_free_min_bytes: 0,
        };
        assert!(evaluate(usage, disabled, &mut state).is_empty());
    }

    #[test]
    fn archive_bytes_sums_only_archive_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("decision_log.jsonl"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.path().join("decision_log.jsonl.20200101-000000"), vec![0u8; 50]).unwrap();
        std::fs::write(dir.path().join("events-20200101.jsonl"), vec![0u8; 999]).unwrap();
        std::fs::write(dir.path().join("wisphive.log.today"), vec![0u8; 999]).unwrap();

        assert_eq!(archive_bytes(dir.path()), 150);
    }

    #[test]
    fn free_bytes_reports_for_real_path() {
        // The temp dir lives on a real filesystem, so statvfs should succeed and
        // report a nonzero figure on any normal CI/dev host.
        let dir = tempfile::tempdir().unwrap();
        let free = free_bytes(dir.path());
        assert!(free.is_some(), "statvfs should succeed for an existing path");
        assert!(free.unwrap() > 0);
    }
}
