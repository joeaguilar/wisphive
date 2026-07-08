use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wisphive_protocol::*;

use wisphive_daemon::DaemonConfig;
use wisphive_daemon::server::Server;
use wisphive_daemon::shutdown;

/// Create a daemon config rooted in a temp directory.
fn temp_config() -> (tempfile::TempDir, DaemonConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let config = DaemonConfig::new(tmp.path().to_path_buf());
    (tmp, config)
}

fn temp_config_without_notifications() -> (tempfile::TempDir, DaemonConfig) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("config.json"), r#"{"notifications":false}"#).unwrap();
    let config = DaemonConfig::new(tmp.path().to_path_buf());
    (tmp, config)
}

/// Helper: connect to the daemon socket and perform handshake as a hook client.
async fn connect_as_hook(
    socket_path: &std::path::Path,
) -> (
    tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let stream = UnixStream::connect(socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send hello
    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Hook,
        version: PROTOCOL_VERSION,
    })
    .unwrap();
    writer.write_all(hello.as_bytes()).await.unwrap();

    // Read welcome
    let welcome_line = lines.next_line().await.unwrap().unwrap();
    let welcome: ServerMessage = decode(&welcome_line).unwrap();
    match welcome {
        ServerMessage::Welcome { version } => assert_eq!(version, PROTOCOL_VERSION),
        other => panic!("expected Welcome, got: {:?}", other),
    }

    (lines, writer)
}

/// Helper: connect to the daemon socket and perform handshake as a TUI client.
async fn connect_as_tui(
    socket_path: &std::path::Path,
) -> (
    tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let (lines, writer, _) = connect_as_tui_with_audit(socket_path).await;
    (lines, writer)
}

async fn connect_as_tui_with_audit(
    socket_path: &std::path::Path,
) -> (
    tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    tokio::net::unix::OwnedWriteHalf,
    Vec<AuditDecision>,
) {
    let stream = UnixStream::connect(socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })
    .unwrap();
    writer.write_all(hello.as_bytes()).await.unwrap();

    // Read welcome
    let welcome_line = lines.next_line().await.unwrap().unwrap();
    let welcome: ServerMessage = decode(&welcome_line).unwrap();
    match welcome {
        ServerMessage::Welcome { .. } => {}
        other => panic!("expected Welcome, got: {:?}", other),
    }

    // Read agents snapshot (sent first)
    let agents_line = lines.next_line().await.unwrap().unwrap();
    let agents: ServerMessage = decode(&agents_line).unwrap();
    match agents {
        ServerMessage::AgentsSnapshot { .. } => {}
        other => panic!("expected AgentsSnapshot, got: {:?}", other),
    }

    // Read initial queue snapshot
    let snap_line = lines.next_line().await.unwrap().unwrap();
    let snap: ServerMessage = decode(&snap_line).unwrap();
    match snap {
        ServerMessage::QueueSnapshot { .. } => {}
        other => panic!("expected QueueSnapshot, got: {:?}", other),
    }

    // Read initial recent-audit snapshot.
    let audit_line = lines.next_line().await.unwrap().unwrap();
    let audit: ServerMessage = decode(&audit_line).unwrap();
    let audit_items = match audit {
        ServerMessage::AuditSnapshot { items } => items,
        other => panic!("expected AuditSnapshot, got: {:?}", other),
    };

    (lines, writer, audit_items)
}

/// Read TUI broadcast messages until we find one matching the predicate.
/// Skips AgentConnected/AgentDisconnected events that the server now emits.
async fn next_tui_msg<F>(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    predicate: F,
) -> ServerMessage
where
    F: Fn(&ServerMessage) -> bool,
{
    loop {
        let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let msg: ServerMessage = decode(&line).unwrap();
        if predicate(&msg) {
            return msg;
        }
        // Skip other broadcast messages (AgentConnected, AgentDisconnected, etc.)
    }
}

fn make_decision_request(tool_name: &str) -> DecisionRequest {
    DecisionRequest {
        id: uuid::Uuid::new_v4(),
        agent_id: "cc-test".into(),
        agent_type: AgentType::ClaudeCode,
        project: PathBuf::from("/test/project"),
        tool_name: tool_name.into(),
        tool_input: serde_json::json!({"command": "cargo test"}),
        timestamp: chrono::Utc::now(),
        hook_event_name: Default::default(),
        tool_use_id: None,
        permission_suggestions: None,
        event_data: None,
        terminal_session_id: None,
    }
}

fn hook_decision_event(
    event: &str,
    tool_name: &str,
    agent_id: &str,
    tool_use_id: &str,
    decided_by: &str,
) -> String {
    serde_json::to_string(&serde_json::json!({
        "event": event,
        "agent_id": agent_id,
        "agent_type": "claude_code",
        "project": "/test/audit",
        "tool_name": tool_name,
        "tool_input": {"path": "/tmp/example"},
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "tool_use_id": tool_use_id,
        "hook_event_name": "PreToolUse",
        "decided_by": decided_by,
    }))
    .unwrap()
}

async fn append_event_line(events_path: &std::path::Path, line: &str) {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path)
        .await
        .unwrap();
    file.write_all(line.as_bytes()).await.unwrap();
    file.write_all(b"\n").await.unwrap();
    file.flush().await.unwrap();
}

/// Start a server in the background, return the shutdown sender and socket path.
async fn start_server(config: DaemonConfig) -> tokio::sync::watch::Sender<bool> {
    let (shutdown_tx, shutdown_rx) = shutdown::shutdown_channel();

    let server = Server::new(config).await.unwrap();
    tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    // Give server a moment to bind the socket
    tokio::time::sleep(Duration::from_millis(50)).await;

    shutdown_tx
}

// ════════════════════════════════════════════════════════════
// Handshake tests
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn hook_handshake_succeeds() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (_lines, _writer) = connect_as_hook(&socket_path).await;

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_handshake_and_empty_snapshot() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    // connect_as_tui already validates Welcome + QueueSnapshot
    let (_lines, _writer) = connect_as_tui(&socket_path).await;

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn wrong_protocol_version_gets_error() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send hello with wrong version
    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Hook,
        version: 999,
    })
    .unwrap();
    writer.write_all(hello.as_bytes()).await.unwrap();

    let response_line = lines.next_line().await.unwrap().unwrap();
    let response: ServerMessage = decode(&response_line).unwrap();
    match response {
        ServerMessage::Error { message } => {
            assert!(message.contains("unsupported protocol version"));
        }
        other => panic!("expected Error, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn non_hello_first_message_gets_error() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send approve instead of hello
    let msg = encode(&ClientMessage::Approve {
        id: uuid::Uuid::new_v4(),
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    writer.write_all(msg.as_bytes()).await.unwrap();

    let response_line = lines.next_line().await.unwrap().unwrap();
    let response: ServerMessage = decode(&response_line).unwrap();
    match response {
        ServerMessage::Error { message } => {
            assert!(message.contains("expected Hello"));
        }
        other => panic!("expected Error, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

// ════════════════════════════════════════════════════════════
// Hook → Daemon → TUI flow
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn hook_sends_request_tui_approves_hook_gets_response() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    // Connect TUI first
    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    // Connect hook and send a decision request
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;
    let req = make_decision_request("Bash");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();

    // TUI should receive the new decision (skip AgentConnected)
    let tui_msg = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;
    match tui_msg {
        ServerMessage::NewDecision(r) => {
            assert_eq!(r.id, req_id);
            assert_eq!(r.tool_name, "Bash");
        }
        other => panic!("expected NewDecision, got: {:?}", other),
    }

    // TUI approves
    let approve = encode(&ClientMessage::Approve {
        id: req_id,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    tui_writer.write_all(approve.as_bytes()).await.unwrap();

    // Hook should receive the decision response
    let hook_resp_line = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let hook_resp: ServerMessage = decode(&hook_resp_line).unwrap();
    match hook_resp {
        ServerMessage::DecisionResponse { id, decision, .. } => {
            assert_eq!(id, req_id);
            assert_eq!(decision, Decision::Approve);
        }
        other => panic!("expected DecisionResponse, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn approve_permission_with_invalid_index_is_rejected_and_stays_pending() {
    // itr#297: a bad suggestion_index must not resolve the request as an
    // approval with no permission actually selected.
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let mut req = make_decision_request("Bash");
    req.hook_event_name = wisphive_protocol::HookEventType::PermissionRequest;
    req.permission_suggestions = Some(vec![]);
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // Invalid index → explicit Error, nothing resolved.
    let bad = encode(&ClientMessage::ApprovePermission {
        id: req_id,
        suggestion_index: 999,
        message: None,
    })
    .unwrap();
    tui_writer.write_all(bad.as_bytes()).await.unwrap();
    let err = next_tui_msg(&mut tui_lines, |m| matches!(m, ServerMessage::Error { .. })).await;
    match err {
        ServerMessage::Error { message } => assert!(message.contains("suggestion_index")),
        other => panic!("expected Error, got: {:?}", other),
    }

    // Still pending: a plain deny resolves it and reaches the hook.
    let deny = encode(&ClientMessage::Deny {
        id: req_id,
        message: None,
    })
    .unwrap();
    tui_writer.write_all(deny.as_bytes()).await.unwrap();
    let hook_resp_line = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let hook_resp: ServerMessage = decode(&hook_resp_line).unwrap();
    assert!(matches!(
        hook_resp,
        ServerMessage::DecisionResponse {
            decision: Decision::Deny,
            ..
        }
    ));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn unfiltered_approve_all_without_confirm_is_rejected() {
    // itr#88: a compromised/buggy client echoing NewDecision events must not
    // blanket-approve the queue with one unconfirmed message.
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let req = make_decision_request("Bash");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // Unconfirmed, unfiltered bulk approve → rejected with an Error.
    let approve_all = encode(&ClientMessage::ApproveAll {
        filter: None,
        confirm: false,
    })
    .unwrap();
    tui_writer.write_all(approve_all.as_bytes()).await.unwrap();
    let err = next_tui_msg(&mut tui_lines, |m| matches!(m, ServerMessage::Error { .. })).await;
    match err {
        ServerMessage::Error { message } => assert!(message.contains("confirm")),
        other => panic!("expected Error, got: {:?}", other),
    }

    // The decision is still pending — approve it properly and the hook gets it.
    let approve = encode(&ClientMessage::Approve {
        id: req_id,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    tui_writer.write_all(approve.as_bytes()).await.unwrap();
    let hook_resp_line = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let hook_resp: ServerMessage = decode(&hook_resp_line).unwrap();
    assert!(matches!(
        hook_resp,
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    // The audit row names the resolving client (itr#88): a local TUI approve
    // is attributed as human:tui, not bare "human".
    let query = encode(&ClientMessage::QueryHistory {
        agent_id: None,
        limit: Some(10),
        request_id: None,
    })
    .unwrap();
    tui_writer.write_all(query.as_bytes()).await.unwrap();
    let history = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::HistoryResponse { .. })
    })
    .await;
    match history {
        ServerMessage::HistoryResponse { entries, .. } => {
            let entry = entries.iter().find(|e| e.id == req_id).expect("audit row");
            assert_eq!(entry.decided_by.as_deref(), Some("human:tui"));
        }
        other => panic!("expected HistoryResponse, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn hook_sends_request_tui_denies_hook_gets_deny() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let req = make_decision_request("Write");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();

    // TUI receives new decision (skip AgentConnected)
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // TUI denies
    let deny = encode(&ClientMessage::Deny {
        id: req_id,
        message: None,
    })
    .unwrap();
    tui_writer.write_all(deny.as_bytes()).await.unwrap();

    // Hook receives deny
    let hook_resp_line = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let hook_resp: ServerMessage = decode(&hook_resp_line).unwrap();
    match hook_resp {
        ServerMessage::DecisionResponse { decision, .. } => {
            assert_eq!(decision, Decision::Deny);
        }
        other => panic!("expected DecisionResponse with Deny, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_receives_decision_resolved_after_approve() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (_hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let req = make_decision_request("Bash");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();

    // TUI receives NewDecision (skip AgentConnected)
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // TUI approves
    let approve = encode(&ClientMessage::Approve {
        id: req_id,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    tui_writer.write_all(approve.as_bytes()).await.unwrap();

    // TUI should also receive DecisionResolved (skip AgentDisconnected)
    let resolved = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::DecisionResolved { .. })
    })
    .await;
    match resolved {
        ServerMessage::DecisionResolved { id, decision } => {
            assert_eq!(id, req_id);
            assert_eq!(decision, Decision::Approve);
        }
        other => panic!("expected DecisionResolved, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn multiple_hooks_queued_then_resolved_individually() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    // Connect two hooks
    let (mut hook1_lines, mut hook1_writer) = connect_as_hook(&socket_path).await;
    let (mut hook2_lines, mut hook2_writer) = connect_as_hook(&socket_path).await;

    let req1 = make_decision_request("Bash");
    let req2 = make_decision_request("Write");
    let id1 = req1.id;
    let id2 = req2.id;

    let msg1 = encode(&ClientMessage::DecisionRequest(req1)).unwrap();
    hook1_writer.write_all(msg1.as_bytes()).await.unwrap();

    let msg2 = encode(&ClientMessage::DecisionRequest(req2)).unwrap();
    hook2_writer.write_all(msg2.as_bytes()).await.unwrap();

    // TUI receives both NewDecision events (skip AgentConnected)
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // Approve hook 2 first (out of order)
    let approve2 = encode(&ClientMessage::Approve {
        id: id2,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    tui_writer.write_all(approve2.as_bytes()).await.unwrap();

    let hook2_resp = tokio::time::timeout(Duration::from_secs(2), hook2_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp2: ServerMessage = decode(&hook2_resp).unwrap();
    assert!(matches!(
        resp2,
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    // Deny hook 1
    let deny1 = encode(&ClientMessage::Deny {
        id: id1,
        message: None,
    })
    .unwrap();
    tui_writer.write_all(deny1.as_bytes()).await.unwrap();

    let hook1_resp = tokio::time::timeout(Duration::from_secs(2), hook1_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp1: ServerMessage = decode(&hook1_resp).unwrap();
    assert!(matches!(
        resp1,
        ServerMessage::DecisionResponse {
            decision: Decision::Deny,
            ..
        }
    ));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn approve_all_resolves_all_pending_hooks() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    let (mut h1_lines, mut h1_writer) = connect_as_hook(&socket_path).await;
    let (mut h2_lines, mut h2_writer) = connect_as_hook(&socket_path).await;
    let (mut h3_lines, mut h3_writer) = connect_as_hook(&socket_path).await;

    for (writer, name) in [
        (&mut h1_writer, "Bash"),
        (&mut h2_writer, "Write"),
        (&mut h3_writer, "Edit"),
    ] {
        let req = make_decision_request(name);
        let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
        writer.write_all(msg.as_bytes()).await.unwrap();
    }

    // Wait for all 3 NewDecision events on TUI (skip AgentConnected)
    for _ in 0..3 {
        let _ = next_tui_msg(&mut tui_lines, |m| {
            matches!(m, ServerMessage::NewDecision(_))
        })
        .await;
    }

    // TUI sends ApproveAll
    let approve_all = encode(&ClientMessage::ApproveAll {
        filter: None,
        confirm: true,
    })
    .unwrap();
    tui_writer.write_all(approve_all.as_bytes()).await.unwrap();

    // All hooks should get Approve
    for lines in [&mut h1_lines, &mut h2_lines, &mut h3_lines] {
        let resp_line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let resp: ServerMessage = decode(&resp_line).unwrap();
        assert!(matches!(
            resp,
            ServerMessage::DecisionResponse {
                decision: Decision::Approve,
                ..
            }
        ));
    }

    let _ = shutdown_tx.send(true);
}

// ════════════════════════════════════════════════════════════
// Error handling
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn hook_sends_non_decision_request_after_hello_gets_error() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut lines, mut writer) = connect_as_hook(&socket_path).await;

    // Send Approve instead of DecisionRequest
    let msg = encode(&ClientMessage::Approve {
        id: uuid::Uuid::new_v4(),
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    writer.write_all(msg.as_bytes()).await.unwrap();

    let resp_line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: ServerMessage = decode(&resp_line).unwrap();
    match resp {
        ServerMessage::Error { message } => {
            assert!(message.contains("expected DecisionRequest"));
        }
        other => panic!("expected Error, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_snapshot_reflects_pending_decisions() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    // Submit a hook request first (no TUI yet)
    let (_hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;
    let req = make_decision_request("Bash");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();

    // Give daemon a moment to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Now connect TUI — snapshot should include the pending decision
    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let hello = encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })
    .unwrap();
    writer.write_all(hello.as_bytes()).await.unwrap();

    // Welcome
    let _ = lines.next_line().await.unwrap().unwrap();

    // Agents snapshot (sent before queue snapshot)
    let _ = lines.next_line().await.unwrap().unwrap();

    // Queue snapshot
    let snap_line = lines.next_line().await.unwrap().unwrap();
    let snap: ServerMessage = decode(&snap_line).unwrap();
    match snap {
        ServerMessage::QueueSnapshot { items } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].id, req_id);
            assert_eq!(items[0].tool_name, "Bash");
        }
        other => panic!("expected QueueSnapshot with 1 item, got: {:?}", other),
    }

    let audit_line = lines.next_line().await.unwrap().unwrap();
    let audit: ServerMessage = decode(&audit_line).unwrap();
    match audit {
        ServerMessage::AuditSnapshot { items } => assert!(items.is_empty()),
        other => panic!("expected AuditSnapshot, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_receives_recent_audit_snapshot_on_connect() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let events_path = config.home_dir.join("events.jsonl");
    let shutdown_tx = start_server(config).await;

    append_event_line(
        &events_path,
        &hook_decision_event(
            "auto_approved",
            "Read",
            "cc-audit",
            "snapshot-1",
            "level:all",
        ),
    )
    .await;

    // Force deterministic ingestion for the snapshot setup. The background tail
    // may also see the line; reimport is idempotent either way.
    let (mut first_lines, mut first_writer) = connect_as_tui(&socket_path).await;
    first_writer
        .write_all(encode(&ClientMessage::ReimportEvents).unwrap().as_bytes())
        .await
        .unwrap();
    let _ = next_tui_msg(&mut first_lines, |m| {
        matches!(m, ServerMessage::ReimportComplete { .. })
    })
    .await;

    let (_lines, _writer, audit_items) = connect_as_tui_with_audit(&socket_path).await;
    let audit = audit_items
        .iter()
        .find(|item| item.agent_id == "cc-audit")
        .expect("recent audit snapshot should include imported auto-approval");
    assert_eq!(audit.kind, AuditDecisionKind::AutoApproved);
    assert_eq!(audit.decided_by.as_deref(), Some("level:all"));
    assert_eq!(audit.project, PathBuf::from("/test/audit"));
    assert_eq!(audit.tool_name, "Read");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_receives_live_audit_decisions_from_events_tail() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let events_path = config.home_dir.join("events.jsonl");
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, _tui_writer) = connect_as_tui(&socket_path).await;

    append_event_line(
        &events_path,
        &hook_decision_event("auto_approved", "Read", "cc-live", "live-1", "level:all"),
    )
    .await;
    let auto = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::AuditDecision(_))
    })
    .await;
    match auto {
        ServerMessage::AuditDecision(audit) => {
            assert_eq!(audit.kind, AuditDecisionKind::AutoApproved);
            assert_eq!(audit.decided_by.as_deref(), Some("level:all"));
            assert_eq!(audit.agent_id, "cc-live");
            assert_eq!(audit.project, PathBuf::from("/test/audit"));
            assert_eq!(audit.tool_name, "Read");
        }
        other => panic!("expected AuditDecision, got: {:?}", other),
    }

    append_event_line(
        &events_path,
        &hook_decision_event(
            "deferred",
            "AskUserQuestion",
            "cc-live",
            "live-2",
            "always_ask:intrinsic",
        ),
    )
    .await;
    let deferred = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::AuditDecision(_))
    })
    .await;
    match deferred {
        ServerMessage::AuditDecision(audit) => {
            assert_eq!(audit.kind, AuditDecisionKind::Deferred);
            assert_eq!(audit.decided_by.as_deref(), Some("always_ask:intrinsic"));
            assert_eq!(audit.agent_id, "cc-live");
            assert_eq!(audit.tool_name, "AskUserQuestion");
        }
        other => panic!("expected AuditDecision, got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

// ════════════════════════════════════════════════════════════
// Shutdown
// ════════════════════════════════════════════════════════════

// ════════════════════════════════════════════════════════════
// Sudo-mode gate (itr#218)
// ════════════════════════════════════════════════════════════
//
// Web-origin approvals of sudo-class tools (Bash/Write/Edit/MultiEdit/
// NotebookEdit/ConfigChange) are held back until the device has been
// marked fresh via MarkDeviceFresh. TUI-origin approvals (device_id =
// None) bypass the gate entirely. Non-sudo tools are not gated.

/// Serialize an Approve wrapped in a ClientCommand envelope tagged with
/// the given device id. Matches what ws_bridge::rewrap_with_device does
/// for browser-origin messages; the daemon dispatch loop sees the same
/// on-wire shape whether the sender is the ws bridge or this test.
fn encode_web_approve(id: uuid::Uuid, device_id: &str) -> String {
    let cmd = ClientCommand::from(ClientMessage::Approve {
        id,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .with_device_id(DeviceId::from(device_id));
    encode(&cmd).unwrap()
}

/// Serialize an ApproveAll wrapped in a ClientCommand envelope tagged
/// with the given device id.
fn encode_web_approve_all(filter: Option<DecisionFilter>, device_id: &str) -> String {
    let cmd = ClientCommand::from(ClientMessage::ApproveAll {
        filter,
        confirm: true,
    })
    .with_device_id(DeviceId::from(device_id));
    encode(&cmd).unwrap()
}

fn encode_web_replay(id: uuid::Uuid, device_id: &str) -> String {
    let cmd = ClientCommand::from(ClientMessage::TermReplay {
        id,
        from_seq: None,
        speed: None,
    })
    .with_device_id(DeviceId::from(device_id));
    encode(&cmd).unwrap()
}

fn terminal_meta(
    id: uuid::Uuid,
    created_by: Option<&str>,
    replay_acl: Vec<&str>,
) -> TerminalSessionMeta {
    TerminalSessionMeta {
        id,
        label: Some("seeded".into()),
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "echo seeded".into()],
        cwd: PathBuf::from("/tmp"),
        cols: 80,
        rows: 24,
        started_at: chrono::Utc::now(),
        ended_at: None,
        exit_code: None,
        status: TerminalStatus::Running,
        group_name: None,
        sort_order: 0,
        created_by: created_by.map(str::to_string),
        replay_acl: replay_acl.into_iter().map(str::to_string).collect(),
    }
}

async fn seed_terminal_history(
    db_path: &std::path::Path,
    created_by: Option<&str>,
    replay_acl: Vec<&str>,
    payloads: Vec<&[u8]>,
) -> uuid::Uuid {
    let db = wisphive_daemon::state::StateDb::open_client(&db_path.to_string_lossy())
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    db.create_terminal_session(&terminal_meta(id, created_by, replay_acl))
        .await
        .unwrap();
    let rows: Vec<_> = payloads
        .into_iter()
        .enumerate()
        .map(|(idx, payload)| {
            (
                id,
                idx as u64,
                idx as i64,
                if idx % 2 == 0 {
                    TerminalDirection::Input
                } else {
                    TerminalDirection::Output
                },
                payload.to_vec(),
            )
        })
        .collect();
    db.insert_terminal_events_batch(&rows).await.unwrap();
    id
}

async fn recent_web_audit(db_path: &std::path::Path) -> Vec<wisphive_daemon::state::WebAuditRow> {
    let db = wisphive_daemon::state::StateDb::open_client(&db_path.to_string_lossy())
        .await
        .unwrap();
    db.list_web_audit(100).await.unwrap()
}

async fn wait_for_web_audit_event(
    db_path: &std::path::Path,
    event: &str,
    session_id: uuid::Uuid,
) -> wisphive_daemon::state::WebAuditRow {
    for _ in 0..20 {
        let rows = recent_web_audit(db_path).await;
        if let Some(row) = rows.into_iter().find(|row| {
            row.event == event
                && row.detail.as_deref().is_some_and(|detail| {
                    serde_json::from_str::<serde_json::Value>(detail)
                        .ok()
                        .and_then(|v| {
                            v["session_id"]
                                .as_str()
                                .map(|s| s == session_id.to_string())
                        })
                        .unwrap_or(false)
                })
        }) {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("missing web_audit event {event} for terminal session {session_id}");
}

#[tokio::test]
async fn term_replay_denies_unauthorized_web_device_and_audits_request() {
    let (_tmp, config) = temp_config_without_notifications();
    let socket_path = config.socket_path.clone();
    let db_path = config.db_path.clone();
    let shutdown_tx = start_server(config).await;

    let session_id = seed_terminal_history(
        &db_path,
        Some("human:web:dev-owner"),
        Vec::new(),
        vec![&b"secret-input"[..], &b"secret-output"[..]],
    )
    .await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    tui_writer
        .write_all(encode_web_replay(session_id, "dev-intruder").as_bytes())
        .await
        .unwrap();

    let err = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::TermError { .. })
    })
    .await;
    match err {
        ServerMessage::TermError {
            id: Some(id),
            message,
        } => {
            assert_eq!(id, session_id);
            assert!(message.contains("replay denied"));
        }
        other => panic!("expected replay TermError, got: {:?}", other),
    }

    let row = wait_for_web_audit_event(&db_path, "terminal_replay", session_id).await;
    assert_eq!(row.device_id.as_deref(), Some("dev-intruder"));
    chrono::DateTime::parse_from_rfc3339(&row.at).expect("audit timestamp is RFC3339");
    let detail: serde_json::Value = serde_json::from_str(row.detail.as_deref().unwrap()).unwrap();
    assert_eq!(detail["session_id"], session_id.to_string());
    assert_eq!(detail["requester"], "human:web:dev-intruder");
    assert_eq!(detail["author"], "human:web:dev-owner");
    assert_eq!(detail["authorized"], false);
    assert_eq!(detail["authorization"], "denied");
    assert_eq!(detail["outcome"], "denied");
    assert_eq!(detail["non_authored"], true);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn term_replay_allows_acl_grant_and_audits_bytes_streamed() {
    use base64::Engine as _;

    let (_tmp, config) = temp_config_without_notifications();
    let socket_path = config.socket_path.clone();
    let db_path = config.db_path.clone();
    let shutdown_tx = start_server(config).await;

    let session_id = seed_terminal_history(
        &db_path,
        Some("human:web:dev-owner"),
        vec!["human:web:dev-reader"],
        vec![&b"abc"[..], &b"wxyz"[..]],
    )
    .await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    tui_writer
        .write_all(encode_web_replay(session_id, "dev-reader").as_bytes())
        .await
        .unwrap();

    let mut events = 0u64;
    let mut bytes = 0usize;
    loop {
        let line = tokio::time::timeout(Duration::from_secs(2), tui_lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match decode::<ServerMessage>(&line).unwrap() {
            ServerMessage::TermReplayChunk {
                id,
                data,
                direction,
                ..
            } => {
                assert_eq!(id, session_id);
                assert!(matches!(
                    direction,
                    TerminalDirection::Input | TerminalDirection::Output
                ));
                events += 1;
                bytes += base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap()
                    .len();
            }
            ServerMessage::TermReplayDone { id, total_events } => {
                assert_eq!(id, session_id);
                assert_eq!(total_events, 2);
                break;
            }
            other => panic!("expected replay chunk/done, got: {:?}", other),
        }
    }
    assert_eq!(events, 2);
    assert_eq!(bytes, 7);

    let request_row = wait_for_web_audit_event(&db_path, "terminal_replay", session_id).await;
    chrono::DateTime::parse_from_rfc3339(&request_row.at).expect("audit timestamp is RFC3339");
    let request_detail: serde_json::Value =
        serde_json::from_str(request_row.detail.as_deref().unwrap()).unwrap();
    assert_eq!(request_detail["requester"], "human:web:dev-reader");
    assert_eq!(request_detail["authorization"], "acl");
    assert_eq!(request_detail["authorized"], true);
    assert_eq!(request_detail["non_authored"], true);
    assert_eq!(request_detail["outcome"], "started");
    assert_eq!(
        request_detail["replay_acl"],
        serde_json::json!(["human:web:dev-reader"])
    );

    let done_row = wait_for_web_audit_event(&db_path, "terminal_replay_done", session_id).await;
    let done_detail: serde_json::Value =
        serde_json::from_str(done_row.detail.as_deref().unwrap()).unwrap();
    assert_eq!(done_detail["requester"], "human:web:dev-reader");
    assert_eq!(done_detail["events"], 2);
    assert_eq!(done_detail["bytes"], 7);
    assert_eq!(done_detail["completed"], true);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn web_origin_bash_approve_without_fresh_reauth_emits_reauth_required() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    // TUI + hook
    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    // Hook submits a Bash decision.
    let req = make_decision_request("Bash");
    let req_id = req.id;
    let msg = encode(&ClientMessage::DecisionRequest(req)).unwrap();
    hook_writer.write_all(msg.as_bytes()).await.unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // Web device tries to approve without having reauthed.
    let approve = encode_web_approve(req_id, "dev-phone");
    tui_writer.write_all(approve.as_bytes()).await.unwrap();

    // TUI bridge receives WebReauthRequired for the offending request.
    let reauth = tokio::time::timeout(Duration::from_secs(2), tui_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match decode::<ServerMessage>(&reauth).unwrap() {
        ServerMessage::WebReauthRequired {
            device_id,
            tool_name,
            ..
        } => {
            assert_eq!(device_id, "dev-phone");
            assert_eq!(tool_name, "Bash");
        }
        other => panic!("expected WebReauthRequired, got: {:?}", other),
    }

    // Hook must still be pending — the daemon did NOT resolve the decision.
    let poll = tokio::time::timeout(Duration::from_millis(200), hook_lines.next_line()).await;
    assert!(
        poll.is_err(),
        "hook should not have received a response while gated"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn mark_device_fresh_then_approve_succeeds() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let req = make_decision_request("Bash");
    let req_id = req.id;
    hook_writer
        .write_all(
            encode(&ClientMessage::DecisionRequest(req))
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // First approve attempt is gated.
    tui_writer
        .write_all(encode_web_approve(req_id, "dev-phone").as_bytes())
        .await
        .unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::WebReauthRequired { .. })
    })
    .await;

    // Mark fresh, wait for ack.
    let mark_fresh = encode(
        &ClientCommand::from(ClientMessage::MarkDeviceFresh)
            .with_device_id(DeviceId::from("dev-phone")),
    )
    .unwrap();
    tui_writer.write_all(mark_fresh.as_bytes()).await.unwrap();
    let ack = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::MarkDeviceFreshAck { .. })
    })
    .await;
    match ack {
        ServerMessage::MarkDeviceFreshAck { device_id } => assert_eq!(device_id, "dev-phone"),
        _ => unreachable!(),
    }

    // Retry approve — should now go through.
    tui_writer
        .write_all(encode_web_approve(req_id, "dev-phone").as_bytes())
        .await
        .unwrap();
    let hook_resp = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match decode::<ServerMessage>(&hook_resp).unwrap() {
        ServerMessage::DecisionResponse {
            id,
            decision: Decision::Approve,
            ..
        } => assert_eq!(id, req_id),
        other => panic!("expected DecisionResponse(Approve), got: {:?}", other),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn mark_device_fresh_without_device_id_is_noop_and_silent() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    // Envelope with no device_id — TUI-local "reauth" has no subject, so
    // the daemon drops it silently (no ack).
    let msg = encode(&ClientCommand::from(ClientMessage::MarkDeviceFresh)).unwrap();
    tui_writer.write_all(msg.as_bytes()).await.unwrap();

    // We should NOT receive an ack.
    let poll = tokio::time::timeout(Duration::from_millis(200), tui_lines.next_line()).await;
    assert!(poll.is_err(), "device_id-less MarkDeviceFresh must not ack");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn tui_origin_bash_approve_is_not_gated() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    let req = make_decision_request("Bash");
    let req_id = req.id;
    hook_writer
        .write_all(
            encode(&ClientMessage::DecisionRequest(req))
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    // Bare ClientMessage = ClientCommand { device_id: None }. Local TUI
    // bypasses the gate — this is the current trust model (see sudo_gate.rs).
    let approve = encode(&ClientMessage::Approve {
        id: req_id,
        message: None,
        updated_input: None,
        always_allow: false,
        additional_context: None,
    })
    .unwrap();
    tui_writer.write_all(approve.as_bytes()).await.unwrap();

    let hook_resp = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode::<ServerMessage>(&hook_resp).unwrap(),
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn web_origin_read_tool_approve_is_not_gated() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;
    let (mut hook_lines, mut hook_writer) = connect_as_hook(&socket_path).await;

    // Read is NOT sudo-class — should pass even without fresh reauth.
    let req = make_decision_request("Read");
    let req_id = req.id;
    hook_writer
        .write_all(
            encode(&ClientMessage::DecisionRequest(req))
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::NewDecision(_))
    })
    .await;

    tui_writer
        .write_all(encode_web_approve(req_id, "dev-phone").as_bytes())
        .await
        .unwrap();

    let hook_resp = tokio::time::timeout(Duration::from_secs(2), hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode::<ServerMessage>(&hook_resp).unwrap(),
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn web_origin_approve_all_partitions_on_sudo_class() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    // Two hooks: one Bash (sudo), one Read (non-sudo).
    let (mut bash_hook_lines, mut bash_writer) = connect_as_hook(&socket_path).await;
    let (mut read_hook_lines, mut read_writer) = connect_as_hook(&socket_path).await;

    let bash_req = make_decision_request("Bash");
    let read_req = make_decision_request("Read");
    let bash_id = bash_req.id;
    bash_writer
        .write_all(
            encode(&ClientMessage::DecisionRequest(bash_req))
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();
    read_writer
        .write_all(
            encode(&ClientMessage::DecisionRequest(read_req))
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();

    for _ in 0..2 {
        let _ = next_tui_msg(&mut tui_lines, |m| {
            matches!(m, ServerMessage::NewDecision(_))
        })
        .await;
    }

    // Web-origin approve_all without fresh reauth.
    tui_writer
        .write_all(encode_web_approve_all(None, "dev-phone").as_bytes())
        .await
        .unwrap();

    // Read hook should resolve.
    let read_resp = tokio::time::timeout(Duration::from_secs(2), read_hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode::<ServerMessage>(&read_resp).unwrap(),
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    // Bash hook should still be pending.
    let bash_poll =
        tokio::time::timeout(Duration::from_millis(200), bash_hook_lines.next_line()).await;
    assert!(bash_poll.is_err(), "Bash should have been held by the gate");

    // TUI bridge should have seen exactly one WebReauthRequired, for Bash.
    let gate_msg = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::WebReauthRequired { .. })
    })
    .await;
    match gate_msg {
        ServerMessage::WebReauthRequired {
            device_id,
            tool_name,
            ..
        } => {
            assert_eq!(device_id, "dev-phone");
            assert_eq!(tool_name, "Bash");
        }
        _ => unreachable!(),
    }

    // Belt-and-suspenders: after MarkDeviceFresh + retry, bash resolves.
    tui_writer
        .write_all(
            encode(
                &ClientCommand::from(ClientMessage::MarkDeviceFresh)
                    .with_device_id(DeviceId::from("dev-phone")),
            )
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::MarkDeviceFreshAck { .. })
    })
    .await;
    tui_writer
        .write_all(encode_web_approve(bash_id, "dev-phone").as_bytes())
        .await
        .unwrap();
    let bash_resp = tokio::time::timeout(Duration::from_secs(2), bash_hook_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode::<ServerMessage>(&bash_resp).unwrap(),
        ServerMessage::DecisionResponse {
            decision: Decision::Approve,
            ..
        }
    ));

    let _ = shutdown_tx.send(true);
}

// ── Cockpit hook-gating: web-driven InstallHooks (itr#460) ──────────
//
// Installing hooks writes .claude/settings.json into a filesystem path from a
// web device, so it is sudo-gated exactly like a Bash/Write approve. A stale
// reauth must bounce WebReauthRequired and write nothing; after MarkDeviceFresh
// the install proceeds and the fresh audit comes back on InstallHooksResult.
#[tokio::test]
async fn web_origin_install_hooks_reauth_gated_then_installs() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().to_path_buf();
    let settings = project_path.join(".claude").join("settings.json");

    let install_cmd = encode(
        &ClientCommand::from(ClientMessage::InstallHooks {
            project: project_path.clone(),
        })
        .with_device_id(DeviceId::from("dev-phone")),
    )
    .unwrap();

    // 1. Stale reauth -> bounced, nothing written.
    tui_writer.write_all(install_cmd.as_bytes()).await.unwrap();
    let reauth = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::WebReauthRequired { .. })
    })
    .await;
    match reauth {
        ServerMessage::WebReauthRequired {
            device_id,
            request_id,
            tool_name,
            ..
        } => {
            assert_eq!(device_id, "dev-phone");
            assert_eq!(tool_name, "InstallHooks");
            // request_id is the project path so the browser can map the retry.
            assert_eq!(request_id, project_path.to_string_lossy());
        }
        _ => unreachable!(),
    }
    assert!(
        !settings.exists(),
        "gated install must not write settings.json"
    );

    // 2. Mark the device fresh, wait for ack.
    let mark_fresh = encode(
        &ClientCommand::from(ClientMessage::MarkDeviceFresh)
            .with_device_id(DeviceId::from("dev-phone")),
    )
    .unwrap();
    tui_writer.write_all(mark_fresh.as_bytes()).await.unwrap();
    let _ = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::MarkDeviceFreshAck { .. })
    })
    .await;

    // 3. Retry -> installs and returns a status with all_installed = true.
    tui_writer.write_all(install_cmd.as_bytes()).await.unwrap();
    let result = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::InstallHooksResult { .. })
    })
    .await;
    match result {
        ServerMessage::InstallHooksResult {
            project: p,
            status,
            error,
        } => {
            assert_eq!(p, project_path);
            assert!(error.is_none(), "install should succeed: {error:?}");
            let status = status.expect("status present on success");
            assert!(status.claude_installed);
            assert!(status.codex_installed);
            assert!(
                status.all_installed,
                "all hooks should be installed: {status:?}"
            );
        }
        _ => unreachable!(),
    }
    assert!(settings.exists(), "install must write settings.json");

    let _ = shutdown_tx.send(true);
}

// A read-only QueryProjectHookStatus is not gated and reflects on-disk state.
#[tokio::test]
async fn query_project_hook_status_reports_disk_state() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    let (mut tui_lines, mut tui_writer) = connect_as_tui(&socket_path).await;

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().to_path_buf();

    // Fresh project: nothing installed.
    let query = encode(&ClientMessage::QueryProjectHookStatus {
        project: project_path.clone(),
    })
    .unwrap();
    tui_writer.write_all(query.as_bytes()).await.unwrap();
    let status = next_tui_msg(&mut tui_lines, |m| {
        matches!(m, ServerMessage::ProjectHookStatus(_))
    })
    .await;
    match status {
        ServerMessage::ProjectHookStatus(s) => {
            assert_eq!(s.project, project_path);
            assert!(!s.claude_installed);
            assert!(!s.all_installed);
            assert!(!s.missing_events.is_empty());
        }
        _ => unreachable!(),
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn server_cleans_up_socket_on_shutdown() {
    let (_tmp, config) = temp_config();
    let socket_path = config.socket_path.clone();
    let shutdown_tx = start_server(config).await;

    // Socket should exist
    assert!(socket_path.exists());

    // Trigger shutdown
    let _ = shutdown_tx.send(true);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Socket should be cleaned up
    assert!(!socket_path.exists());
}
