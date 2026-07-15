//! itr#535 — every config/perms denial is actionable, and fail-closed stays.
//!
//! Posture (ADR-0010, PO-endorsed after the 2026-07-15 incident): a
//! missing/unreadable/unsafe mode file denies EVERY event type — including
//! human-origin ones like UserPromptSubmit — and the repair channel is the
//! denial *message*, never a fail-open branch. These tests matrix event types
//! against failure classes and assert each denial carries all three actionable
//! elements: the failing path, the required state, and the exact repair
//! commands (`chmod`/`chown` line, `wisphive doctor --fix-perms`,
//! `scripts/wisphive-rescue.sh`, `wisphive emergency-off`).
//!
//! If a change makes one of these cells exit 0, that is a fail-open hole in a
//! deliberate security posture — fix the change, not the test.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const EVENT_TYPES: [&str; 4] = [
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "PermissionRequest",
];

/// Drive the real hook binary with an isolated HOME. The mode check runs
/// before stdin is read, so the child may exit before consuming the pipe —
/// the stdin write is therefore best-effort (EPIPE is expected, not a bug).
fn run_hook_event(home: &Path, event_name: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wisphive-hook"))
        .env("HOME", home)
        .env_remove("WISPHIVE_AGENT_TYPE")
        .env_remove("WISPHIVE_TERMINAL_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wisphive-hook");

    let payload = serde_json::json!({
        "hook_event_name": event_name,
        "session_id": "itr535-matrix",
        "tool_name": "Bash",
        "tool_input": {"command": "echo hi"},
        "cwd": home,
    });
    let _ = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.to_string().as_bytes());

    child.wait_with_output().expect("wait for wisphive-hook")
}

/// Assert one matrix cell: DENY (exit 2, no stdout) with an actionable reason
/// naming (1) the failing path, (2) the required state, (3) the exact repair
/// commands, plus the class-specific diagnostic fragment.
fn assert_actionable_deny(home: &Path, event_name: &str, class: &str, class_fragment: &str) {
    let output = run_hook_event(home, event_name);
    let case = format!("{class} x {event_name}");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Outcome: still fail-closed, for every event type. Never weaken this.
    assert_eq!(
        output.status.code(),
        Some(2),
        "{case}: must DENY: {output:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "{case}: pre-parse denial must not emit event JSON: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    // Element 1: the failing file/path.
    let dir = home.join(".wisphive");
    let mode_path = dir.join("mode");
    assert!(
        stderr.contains(&dir.display().to_string())
            && stderr.contains(&mode_path.display().to_string()),
        "{case}: denial must name the failing paths: {stderr}"
    );
    assert!(
        stderr.contains(class_fragment),
        "{case}: denial must carry the class-specific diagnosis {class_fragment:?}: {stderr}"
    );

    // Element 2: the required state (perms / ownership / non-symlink).
    for required in ["mode 0700", "mode 0600", "non-symlink", "owned by"] {
        assert!(
            stderr.contains(required),
            "{case}: denial must state the required state ({required:?}): {stderr}"
        );
    }

    // Element 3: the exact repair commands, ending at the emergency exit.
    for repair in [
        "chmod 700",
        "chmod 600",
        "chown",
        "wisphive doctor --fix-perms",
        "scripts/wisphive-rescue.sh",
        "wisphive emergency-off",
    ] {
        assert!(
            stderr.contains(repair),
            "{case}: denial must carry the repair command {repair:?}: {stderr}"
        );
    }
}

fn state_dir(home: &Path) -> PathBuf {
    let dir = home.join(".wisphive");
    std::fs::create_dir(&dir).expect("create .wisphive");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn write_mode(dir: &Path, mode_bits: u32) -> PathBuf {
    let path = dir.join("mode");
    std::fs::write(&path, "active\n").expect("write mode file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode_bits)).unwrap();
    path
}

#[test]
fn overpermissive_mode_file_denies_every_event_with_repair_path() {
    let home = tempfile::tempdir().unwrap();
    let dir = state_dir(home.path());
    write_mode(&dir, 0o644);

    for event in EVENT_TYPES {
        assert_actionable_deny(home.path(), event, "mode-file 0644", "found mode 0644");
    }
}

#[test]
fn overpermissive_state_dir_denies_every_event_with_repair_path() {
    let home = tempfile::tempdir().unwrap();
    let dir = state_dir(home.path());
    write_mode(&dir, 0o600);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    for event in EVENT_TYPES {
        assert_actionable_deny(home.path(), event, "state-dir 0755", "found mode 0755");
    }
}

#[test]
fn symlinked_mode_file_denies_every_event_with_repair_path() {
    let home = tempfile::tempdir().unwrap();
    let dir = state_dir(home.path());
    let target = home.path().join("real-mode");
    std::fs::write(&target, "active\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&target, dir.join("mode")).unwrap();

    for event in EVENT_TYPES {
        assert_actionable_deny(
            home.path(),
            event,
            "symlinked mode",
            "cannot open mode file",
        );
    }
}

#[test]
fn symlinked_state_dir_denies_every_event_with_repair_path() {
    let home = tempfile::tempdir().unwrap();
    let real = home.path().join("real-state");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mode = real.join("mode");
    std::fs::write(&mode, "active\n").unwrap();
    std::fs::set_permissions(&mode, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&real, home.path().join(".wisphive")).unwrap();

    for event in EVENT_TYPES {
        assert_actionable_deny(
            home.path(),
            event,
            "symlinked state dir",
            "cannot open state directory",
        );
    }
}

#[test]
fn missing_mode_file_denies_every_event_with_repair_path() {
    let home = tempfile::tempdir().unwrap();
    state_dir(home.path());

    for event in EVENT_TYPES {
        assert_actionable_deny(home.path(), event, "missing mode", "cannot open mode file");
    }
}

// Foreign-owner (uid mismatch) is intentionally NOT matrixed here: creating a
// file owned by another uid requires root, which the test environment does not
// have. The ownership check shares the exact code path asserted above (the
// `uid() != effective_uid` arm of the same conditionals that produce the
// "found mode …, owner uid …" diagnostics), and the unit test
// `missing_mode_produces_a_generic_pre_parse_deny` pins the composed message.
