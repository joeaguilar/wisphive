//! itr#559 env-wiring proof: a child hook spawned with
//! `WISPHIVE_PROMPT_SURFACE=headless` (exactly what the daemon's
//! `build_agent_command` sets on managed spawns) must resolve an intrinsic
//! always-defer tool as a deterministic fail-closed deny-with-reason — never
//! the `"ask"` that blocks a headless `claude -p` silently — and audit it to
//! events.jsonl. This covers `prompt_surface_from_env` through the real
//! process boundary, which the in-process unit tests cannot (env is
//! process-global and must not be mutated in parallel tests).

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
#[cfg(unix)]
fn headless_marker_denies_intrinsic_defer_with_audit() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".wisphive");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mode = state.join("mode");
    std::fs::write(&mode, "active").unwrap();
    std::fs::set_permissions(&mode, std::fs::Permissions::from_mode(0o600)).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wisphive-hook"))
        .env("HOME", home.path())
        .env("WISPHIVE_PROMPT_SURFACE", "headless")
        // Prove the marker alone drives the classification, regardless of the
        // spawning environment (a wisphive-gated dev session could carry
        // either variable).
        .env_remove("WISPHIVE_AGENT_TYPE")
        .env_remove("WISPHIVE_AGENT_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wisphive-hook");

    let payload = serde_json::json!({
        "session_id": "it559",
        "hook_event_name": "PreToolUse",
        "tool_name": "AskUserQuestion",
        "tool_input": {"question": "ship?"},
        "tool_use_id": "it559-1",
        "cwd": "/tmp/p"
    });
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write hook payload");
    let output = child.wait_with_output().expect("wait for wisphive-hook");

    // Deterministic deny-with-reason on stdout (exit 0 — the JSON controls
    // the decision), never `"ask"` (the probe-verified silent block under
    // headless `claude -p`, docs/research/headless-ask-probe/).
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("the headless resolution must emit decision JSON");
    assert_eq!(
        json["hookSpecificOutput"]["permissionDecision"], "deny",
        "a headless intrinsic defer must fail closed, not ask: {stdout}"
    );
    let reason = json["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("the deny must carry an operator-readable reason");
    assert!(
        reason.contains("AskUserQuestion"),
        "names the tool: {reason}"
    );
    assert!(
        reason.contains("HEADLESS"),
        "names the promptless origin: {reason}"
    );

    // Audited with the promptless-class attribution — the record the daemon
    // ingests into decision_log so it reaches `wisphive audit` (B-A9).
    let events = std::fs::read_to_string(state.join("events.jsonl"))
        .expect("the fail-closed resolution must write an events.jsonl record");
    let record: serde_json::Value = serde_json::from_str(events.lines().next().unwrap()).unwrap();
    assert_eq!(record["event"], "denied");
    assert_eq!(
        record["decided_by"],
        "always_ask:headless_no_prompt:intrinsic"
    );
    assert_eq!(record["tool_name"], "AskUserQuestion");
    assert_eq!(record["tool_use_id"], "it559-1");
    assert_eq!(record["hook_event_name"], "PreToolUse");
}
