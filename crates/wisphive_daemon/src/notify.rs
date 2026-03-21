use wisphive_protocol::DecisionRequest;

/// Send a macOS notification for a pending decision.
/// Non-blocking — spawns the notification in the background.
pub fn notify_decision(req: &DecisionRequest) {
    let project_name = req
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let title = format!("Wisphive: {} needs approval", req.tool_name);
    let body = format!(
        "{} in {} ({})",
        tool_summary(req),
        project_name,
        req.agent_id
    );

    // Fire and forget — don't block the daemon
    tokio::spawn(async move {
        send_notification(&title, &body).await;
    });
}

/// Platform-specific notification delivery.
async fn send_notification(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"Tink\"",
            escape_applescript(body),
            escape_applescript(title)
        );

        let _ = tokio::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On Linux, try notify-send
        let _ = tokio::process::Command::new("notify-send")
            .args([title, body])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}

/// Build a compact summary of the tool input.
fn tool_summary(req: &DecisionRequest) -> String {
    if let Some(cmd) = req.tool_input.get("command").and_then(|v| v.as_str()) {
        let truncated = if cmd.len() > 60 {
            format!("{}...", &cmd[..57])
        } else {
            cmd.to_string()
        };
        return truncated;
    }
    if let Some(path) = req.tool_input.get("file_path").and_then(|v| v.as_str()) {
        return path.to_string();
    }
    if let Some(pattern) = req.tool_input.get("pattern").and_then(|v| v.as_str()) {
        return pattern.to_string();
    }
    req.tool_name.clone()
}

/// Escape special characters for AppleScript strings.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
