use tracing::{info, warn};
use wisphive_protocol::DecisionRequest;

/// Send a passive notification for a pending decision.
///
/// On macOS this uses `display notification` (non-intrusive banner).
/// On Linux this uses `notify-send`.
/// The notification body includes all tool input details so the user
/// has full context when they switch to the TUI to respond.
pub fn notify_decision(req: &DecisionRequest) {
    let project_name = req
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let title = format!("Wisphive: {} needs approval", req.tool_name);
    let body = format!(
        "{}\n\nProject: {} ({})",
        tool_input_summary(req),
        project_name,
        req.agent_id
    );

    tokio::spawn(async move {
        if let Err(e) = send_passive_notification(&title, &body).await {
            warn!("failed to send notification: {e}");
        } else {
            info!("sent passive notification: {title}");
        }
    });
}

/// Show a platform-specific passive notification.
async fn send_passive_notification(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(body),
            escape_applescript(title)
        );

        let status = tokio::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| e.to_string())?;

        if !status.success() {
            return Err("osascript exited with non-zero status".into());
        }
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let status = tokio::process::Command::new("notify-send")
            .args([title, body])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| e.to_string())?;

        if !status.success() {
            return Err("notify-send exited with non-zero status".into());
        }
        Ok(())
    }
}

/// Build a full summary of all tool input fields.
///
/// Each key-value pair is rendered on its own line so the notification
/// body shows everything Claude Code is presenting.
fn tool_input_summary(req: &DecisionRequest) -> String {
    if let Some(obj) = req.tool_input.as_object() {
        if obj.is_empty() {
            return req.tool_name.clone();
        }
        obj.iter()
            .map(|(k, v)| {
                let val = match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                };
                format!("{k}: {val}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        req.tool_name.clone()
    }
}

/// Escape special characters for AppleScript strings.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
