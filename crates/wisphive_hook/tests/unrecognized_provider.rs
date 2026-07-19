//! itr#562 pre-parse corner: an unrecognized `WISPHIVE_AGENT_TYPE` combined
//! with a pre-parse failure (here: oversized stdin, whose deny is absolute and
//! independent of `fail-mode`) must use the provider-agnostic bare exit-2
//! channel — NOT the Claude-shaped deny JSON on stdout, which an unknown
//! provider may ignore and thereby turn the strongest deny into an effective
//! allow. The resolution must also land in events.jsonl (minimal record: no
//! tool context exists before the payload parses).

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
#[cfg(unix)]
fn unrecognized_provider_with_oversized_stdin_uses_bare_exit_channel() {
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
        .env("WISPHIVE_AGENT_TYPE", "gemini")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wisphive-hook");

    // Exceed MAX_STDIN_BYTES (8 MiB). The hook stops reading at the cap, so
    // later writes may hit a closed pipe — that is the expected shutdown, not
    // a test failure.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let chunk = vec![b'x'; 1024 * 1024];
    for _ in 0..9 {
        if stdin.write_all(&chunk).is_err() {
            break;
        }
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait for wisphive-hook");

    // Bare-exit deny: exit 2, stderr message, and crucially NO stdout — a
    // Claude-shaped JSON deny here would be the residual form of the itr#562
    // bug (an unknown provider ignoring unparseable stdout and proceeding).
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "the refusal must not emit provider-shaped stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("8 MiB"),
        "names the oversized cause: {stderr}"
    );
    assert!(
        stderr.contains("WISPHIVE_AGENT_TYPE"),
        "names the unidentified provider: {stderr}"
    );
    assert!(
        !stderr.contains("hookSpecificOutput"),
        "no JSON dialect anywhere: {stderr}"
    );

    // Audited, never silent: minimal pre-parse record with the itr#562
    // attribution (no tool context exists before the payload parses).
    let events = std::fs::read_to_string(state.join("events.jsonl"))
        .expect("the pre-parse refusal must write an events.jsonl record");
    let record: serde_json::Value = serde_json::from_str(events.lines().next().unwrap()).unwrap();
    assert_eq!(record["event"], "denied");
    assert_eq!(record["decided_by"], "agent_type:unrecognized");
    assert_eq!(record["agent_type"], "unrecognized:gemini");
    assert_eq!(record["tool_name"], "pre-parse");
    assert!(
        record["tool_input"]["pre_parse_failure"]
            .as_str()
            .unwrap()
            .contains("8 MiB")
    );
}
