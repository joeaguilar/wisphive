use tracing::{info, warn};
use wisphive_protocol::DecisionRequest;

/// Send a passive notification for a pending decision.
///
/// On macOS, prefers `terminal-notifier` (clicking the notification focuses the
/// terminal running the TUI). Falls back to `display notification` via osascript.
/// On Linux, uses `notify-send`.
///
/// The notification body includes all tool input details so the user
/// has full context when they switch to the TUI to respond.
pub fn notify_decision(req: &DecisionRequest) {
    let project_name = req
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let title = if req.permission_suggestions.is_some() {
        format!("Wisphive: {} permission request", req.tool_name)
    } else {
        format!("Wisphive: {} needs approval", req.tool_name)
    };
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
        return send_macos_notification(title, body).await;
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

/// macOS notification with click-to-focus support.
///
/// Tries `terminal-notifier` first — clicking the notification activates the
/// user's terminal app (where the TUI runs). Falls back to osascript
/// `display notification` if `terminal-notifier` is not installed.
#[cfg(target_os = "macos")]
async fn send_macos_notification(title: &str, body: &str) -> Result<(), String> {
    let bundle_id = terminal_bundle_id();

    // Try terminal-notifier (click-to-focus support)
    let tn_result = tokio::process::Command::new("terminal-notifier")
        .args([
            "-title",
            title,
            "-message",
            body,
            "-activate",
            &bundle_id,
            "-group",
            "wisphive",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    match tn_result {
        Ok(status) if status.success() => return Ok(()),
        _ => {
            // Fall back to osascript display notification
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
            Ok(())
        }
    }
}

/// Detect the terminal app's bundle ID for click-to-activate.
///
/// Checks `WISPHIVE_TERMINAL_BUNDLE_ID` env var first, then `TERM_PROGRAM`,
/// then defaults to Terminal.app.
#[cfg(target_os = "macos")]
fn terminal_bundle_id() -> String {
    if let Ok(id) = std::env::var("WISPHIVE_TERMINAL_BUNDLE_ID") {
        return id;
    }

    match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("iTerm.app") => "com.googlecode.iterm2".into(),
        Ok("Alacritty") => "org.alacritty".into(),
        Ok("kitty") => "net.kovidgoyal.kitty".into(),
        Ok("WarpTerminal") => "dev.warp.Warp-Stable".into(),
        Ok("ghostty") => "com.mitchellh.ghostty".into(),
        _ => "com.apple.Terminal".into(),
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
