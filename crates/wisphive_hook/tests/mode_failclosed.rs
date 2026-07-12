use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run_permission_request(home: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wisphive-hook"))
        .env("HOME", home)
        .env("WISPHIVE_AGENT_TYPE", "codex")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wisphive-hook");

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(
            serde_json::json!({
                "hook_event_name": "PermissionRequest",
                "session_id": "mode-regression",
                "tool_name": "Bash",
                "tool_input": {"command": "cargo test"},
                "cwd": home,
            })
            .to_string()
            .as_bytes(),
        )
        .expect("write PermissionRequest input");

    child.wait_with_output().expect("wait for wisphive-hook")
}

fn assert_generic_mode_deny(output: &Output, case: &str) {
    assert_eq!(output.status.code(), Some(2), "{case}: {output:?}");
    assert!(output.stdout.is_empty(), "{case}: stdout must stay empty");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mode file is missing, unreadable, or unsafe"),
        "{case}: {stderr}"
    );
    assert!(
        !stderr.contains("PreToolUse") && !stderr.contains("hookSpecificOutput"),
        "{case}: pre-parse denial must not claim an event-specific JSON shape: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn permission_request_with_missing_or_unsafe_mode_exits_two_without_event_json() {
    use std::os::unix::fs::PermissionsExt;

    let missing_home = tempfile::tempdir().unwrap();
    let missing_state = missing_home.path().join(".wisphive");
    std::fs::create_dir(&missing_state).unwrap();
    std::fs::set_permissions(&missing_state, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_generic_mode_deny(&run_permission_request(missing_home.path()), "missing mode");

    let unsafe_home = tempfile::tempdir().unwrap();
    let unsafe_state = unsafe_home.path().join(".wisphive");
    std::fs::create_dir(&unsafe_state).unwrap();
    std::fs::set_permissions(&unsafe_state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let unsafe_mode = unsafe_state.join("mode");
    std::fs::write(&unsafe_mode, "active").unwrap();
    std::fs::set_permissions(&unsafe_mode, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_generic_mode_deny(&run_permission_request(unsafe_home.path()), "unsafe mode");
}
