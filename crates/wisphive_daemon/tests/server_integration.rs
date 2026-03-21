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

    (lines, writer)
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
    }
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
    let approve = encode(&ClientMessage::Approve { id: req_id, message: None, updated_input: None, always_allow: false, additional_context: None }).unwrap();
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
    let deny = encode(&ClientMessage::Deny { id: req_id, message: None }).unwrap();
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
    let approve = encode(&ClientMessage::Approve { id: req_id, message: None, updated_input: None, always_allow: false, additional_context: None }).unwrap();
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
    let approve2 = encode(&ClientMessage::Approve { id: id2, message: None, updated_input: None, always_allow: false, additional_context: None }).unwrap();
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
    let deny1 = encode(&ClientMessage::Deny { id: id1, message: None }).unwrap();
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
    let approve_all = encode(&ClientMessage::ApproveAll { filter: None }).unwrap();
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

    let _ = shutdown_tx.send(true);
}

// ════════════════════════════════════════════════════════════
// Shutdown
// ════════════════════════════════════════════════════════════

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
