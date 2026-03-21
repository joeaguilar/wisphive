use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use wisphive_protocol::{
    ClientMessage, ClientType, HistorySearch, ServerMessage, PROTOCOL_VERSION,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

fn connect_to_daemon() -> Result<(BufReader<UnixStream>, UnixStream)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let socket_path = PathBuf::from(home).join(".wisphive").join("wisphive.sock");

    let stream = UnixStream::connect(&socket_path)
        .context("could not connect to daemon — is it running? (wisphive daemon start)")?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let hello = wisphive_protocol::encode(&ClientMessage::Hello {
        client: ClientType::Tui,
        version: PROTOCOL_VERSION,
    })?;
    writer.write_all(hello.as_bytes())?;

    let mut welcome_line = String::new();
    reader.read_line(&mut welcome_line)?;
    let msg: ServerMessage = wisphive_protocol::decode(&welcome_line)?;
    match msg {
        ServerMessage::Welcome { .. } => {}
        ServerMessage::Error { message } => anyhow::bail!("daemon error: {message}"),
        _ => anyhow::bail!("unexpected response from daemon"),
    }

    Ok((reader, writer))
}

fn send_and_recv(msg: &ClientMessage) -> Result<ServerMessage> {
    let (mut reader, mut writer) = connect_to_daemon()?;

    // Drain the initial QueueSnapshot
    let mut snapshot_line = String::new();
    reader.read_line(&mut snapshot_line)?;

    let encoded = wisphive_protocol::encode(msg)?;
    writer.write_all(encoded.as_bytes())?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    let response: ServerMessage = wisphive_protocol::decode(&response_line)?;
    Ok(response)
}

/// Search the audit history.
pub async fn search(
    query: String,
    agent_id: Option<String>,
    tool: Option<String>,
    limit: u32,
) -> Result<()> {
    let search = HistorySearch {
        query: Some(query),
        agent_id,
        tool_name: tool,
        limit: Some(limit),
    };

    let response = send_and_recv(&ClientMessage::SearchHistory(search))?;
    match response {
        ServerMessage::HistoryResponse { entries } => {
            if entries.is_empty() {
                println!("No matching history entries found.");
                return Ok(());
            }
            print_entries(&entries);
        }
        ServerMessage::Error { message } => anyhow::bail!("search failed: {message}"),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
    Ok(())
}

/// Show recent history entries.
pub async fn recent(limit: u32, agent_id: Option<String>) -> Result<()> {
    let response = send_and_recv(&ClientMessage::QueryHistory {
        agent_id,
        limit: Some(limit),
    })?;

    match response {
        ServerMessage::HistoryResponse { entries } => {
            if entries.is_empty() {
                println!("No history entries found.");
                return Ok(());
            }
            print_entries(&entries);
        }
        ServerMessage::Error { message } => anyhow::bail!("query failed: {message}"),
        _ => anyhow::bail!("unexpected response from daemon"),
    }
    Ok(())
}

fn print_entries(entries: &[wisphive_protocol::HistoryEntry]) {
    println!(
        "{:<10} {:<10} {:<14} {:<18} {:<50}",
        "DECISION", "TOOL", "AGENT", "TIME", "DETAIL"
    );
    println!("{}", "-".repeat(102));

    for entry in entries {
        let decision = match entry.decision {
            wisphive_protocol::Decision::Approve => "APPROVED",
            wisphive_protocol::Decision::Deny => "DENIED",
            wisphive_protocol::Decision::Ask => "DEFERRED",
        };

        let time_str = entry.resolved_at.format("%m-%d %H:%M:%S").to_string();

        let detail = extract_detail(entry);

        let result_mark = if entry.tool_result.is_some() {
            "+"
        } else {
            " "
        };

        println!(
            "{:<10} {:<10} {:<14} {:<18} {}{}",
            decision, entry.tool_name, entry.agent_id, time_str, result_mark, detail
        );
    }
}

fn extract_detail(entry: &wisphive_protocol::HistoryEntry) -> String {
    if let Some(cmd) = entry.tool_input.get("command").and_then(|v| v.as_str()) {
        return truncate(cmd, 48);
    }
    if let Some(path) = entry.tool_input.get("file_path").and_then(|v| v.as_str()) {
        return path.to_string();
    }
    if let Some(pattern) = entry.tool_input.get("pattern").and_then(|v| v.as_str()) {
        return truncate(pattern, 48);
    }
    entry.tool_name.clone()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}
