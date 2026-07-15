//! itr#473 — the null-id PermissionRequest twin must not spawn an orphan
//! deferred audit record.
//!
//! Claude Code fires BOTH a PreToolUse and a PermissionRequest for an
//! always-defer tool (AskUserQuestion / ExitPlanMode). The PreToolUse event
//! carries the real `tool_use_id`; the PermissionRequest twin carries none, so
//! the answered-signal (PostToolUse ToolResult correlated by tool_use_id,
//! itr#461) can never resolve a row it creates — that row sticks in the
//! Command Center inbox forever. The hook therefore audits ONLY the PreToolUse
//! defer for Claude Code, while still returning Decision::Ask on both events
//! so the native dialog renders (itr#388 must not regress).
//!
//! Codex is different: it has no PreToolUse twin (its PreToolUse "ask" is a
//! fail-closed deny, itr#366) — its PermissionRequest IS the canonical native
//! approval path, so its deferred audit record must keep being written.
//!
//! These tests drive the real binary with an isolated HOME, the exact gap
//! (itr#463 "live drive not yet run") that let the original bug ship green.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Build an isolated `$HOME/.wisphive` satisfying the hook's descriptor-based
/// mode-file checks: dir 0700, `mode` regular file 0600 containing `active`.
fn active_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("create temp HOME");
    let state = home.path().join(".wisphive");
    std::fs::create_dir(&state).expect("create .wisphive");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mode = state.join("mode");
    std::fs::write(&mode, "active\n").expect("write mode file");
    std::fs::set_permissions(&mode, std::fs::Permissions::from_mode(0o600)).unwrap();
    home
}

fn run_hook(home: &Path, agent_env: Option<&str>, stdin_json: serde_json::Value) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wisphive-hook"));
    cmd.env("HOME", home)
        .env_remove("WISPHIVE_AGENT_TYPE")
        .env_remove("WISPHIVE_TERMINAL_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(agent) = agent_env {
        cmd.env("WISPHIVE_AGENT_TYPE", agent);
    }
    let mut child = cmd.spawn().expect("spawn wisphive-hook");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_json.to_string().as_bytes())
        .expect("write hook stdin");
    child.wait_with_output().expect("wait for wisphive-hook")
}

/// Parse every `"event":"deferred"` record out of `$HOME/.wisphive/events.jsonl`
/// (an absent file means zero records — the hook creates it lazily on append).
fn deferred_records(home: &Path) -> Vec<serde_json::Value> {
    let path = home.join(".wisphive").join("events.jsonl");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid jsonl"))
        .filter(|record| record.get("event").and_then(|v| v.as_str()) == Some("deferred"))
        .collect()
}

fn ask_user_question_input() -> serde_json::Value {
    serde_json::json!({
        "questions": [{
            "question": "Which option?",
            "options": [{"label": "A"}, {"label": "B"}]
        }]
    })
}

fn claude_pre_tool_use(home: &Path) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "itr473-e2e",
        "tool_name": "AskUserQuestion",
        "tool_use_id": "toolu_itr473",
        "tool_input": ask_user_question_input(),
        "cwd": home,
    })
}

/// The live twin: same call, no tool_use_id, permission_suggestions present.
fn claude_permission_request(home: &Path) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "PermissionRequest",
        "session_id": "itr473-e2e",
        "tool_name": "AskUserQuestion",
        "tool_input": ask_user_question_input(),
        "permission_suggestions": [{
            "behavior": "allow",
            "destination": "session",
            "rules": [{"toolName": "AskUserQuestion"}]
        }],
        "cwd": home,
    })
}

#[test]
fn claude_pretooluse_defer_is_audited_once() {
    let home = active_home();

    let output = run_hook(home.path(), None, claude_pre_tool_use(home.path()));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("PreToolUse ask JSON");
    assert_eq!(
        stdout["hookSpecificOutput"]["permissionDecision"], "ask",
        "always-defer must hand AskUserQuestion to the native prompt"
    );

    let records = deferred_records(home.path());
    assert_eq!(records.len(), 1, "exactly one deferred audit record");
    let record = &records[0];
    assert_eq!(record["hook_event_name"], "PreToolUse");
    assert_eq!(record["tool_use_id"], "toolu_itr473");
    assert_eq!(record["decided_by"], "always_ask:intrinsic");
}

#[test]
fn claude_permission_request_twin_returns_ask_without_duplicate_audit() {
    let home = active_home();

    let output = run_hook(home.path(), None, claude_permission_request(home.path()));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Decision::Ask emits NO decision object on PermissionRequest — empty
    // stdout lets Claude's native dialog render and capture the selection
    // (itr#388). A behavior:allow here would silently resolve the prompt.
    assert!(
        output.stdout.is_empty(),
        "Ask on PermissionRequest must emit no decision object: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    // itr#473: the null-tool_use_id twin must NOT leave a deferred audit
    // record — nothing could ever resolve the inbox row it would spawn.
    assert_eq!(
        deferred_records(home.path()).len(),
        0,
        "Claude PermissionRequest twin must not write a deferred record"
    );
}

#[test]
fn claude_twin_pair_yields_exactly_one_resolvable_record() {
    let home = active_home();

    // Claude Code fires both events for the same AskUserQuestion call.
    let pre = run_hook(home.path(), None, claude_pre_tool_use(home.path()));
    assert_eq!(pre.status.code(), Some(0), "{pre:?}");
    let perm = run_hook(home.path(), None, claude_permission_request(home.path()));
    assert_eq!(perm.status.code(), Some(0), "{perm:?}");

    let records = deferred_records(home.path());
    assert_eq!(
        records.len(),
        1,
        "the twin pair must leave exactly ONE deferred record: {records:?}"
    );
    // ... and it is the resolvable one: the PreToolUse record whose
    // tool_use_id the PostToolUse answered-signal (itr#461) can correlate.
    assert_eq!(records[0]["hook_event_name"], "PreToolUse");
    assert_eq!(records[0]["tool_use_id"], "toolu_itr473");
}

#[test]
fn codex_permission_request_defer_audit_is_preserved() {
    let home = active_home();

    // Codex-shaped PermissionRequest: no PreToolUse twin exists, so this
    // record is the canonical audit entry and must keep being written.
    let output = run_hook(
        home.path(),
        Some("codex"),
        serde_json::json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "itr473-codex",
            "tool_name": "AskUserQuestion",
            "tool_input": ask_user_question_input(),
            "cwd": home.path(),
        }),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "Ask on Codex PermissionRequest emits no decision object: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    let records = deferred_records(home.path());
    assert_eq!(records.len(), 1, "Codex deferred audit must be preserved");
    let record = &records[0];
    assert_eq!(record["hook_event_name"], "PermissionRequest");
    assert_eq!(record["agent_type"], "codex");
    assert_eq!(record["decided_by"], "always_ask:intrinsic");
}
