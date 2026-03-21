/// Panel rendering for the Wisphive TUI.
///
/// Three main panels:
/// - Queue: pending decision requests
/// - Agents: connected agent instances
/// - Projects: per-project status summary
///
/// Rendering is done in ui.rs using these as data sources.
/// This module provides helper types and formatting for panel content.
use wisphive_protocol::DecisionRequest;

use crate::app::ProjectStatus;

/// Format a decision request as a compact single-line string for the queue panel.
pub fn format_queue_item(req: &DecisionRequest) -> String {
    let project_name = req
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| req.project.to_string_lossy().to_string());

    let age = chrono::Utc::now()
        .signed_duration_since(req.timestamp)
        .num_seconds();

    let age_str = if age < 60 {
        format!("{age}s")
    } else if age < 3600 {
        format!("{}m", age / 60)
    } else {
        format!("{}h", age / 3600)
    };

    let prefix = match req.hook_event_name {
        wisphive_protocol::HookEventType::PermissionRequest => "P ",
        wisphive_protocol::HookEventType::Elicitation => "E ",
        wisphive_protocol::HookEventType::Stop | wisphive_protocol::HookEventType::SubagentStop => "S ",
        wisphive_protocol::HookEventType::UserPromptSubmit => "U ",
        wisphive_protocol::HookEventType::ConfigChange => "C ",
        wisphive_protocol::HookEventType::TeammateIdle => "T ",
        wisphive_protocol::HookEventType::TaskCompleted => "D ",
        _ => "  ",
    };

    format!(
        "{}[{}] {:<12} {:<8} {}  {:>5}",
        prefix,
        req.agent_id,
        project_name,
        req.tool_name,
        truncate_tool_input(req),
        age_str
    )
}

/// Format a project status line for the projects panel.
pub fn format_project_status(status: &ProjectStatus) -> String {
    let name = status
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| status.path.to_string_lossy().to_string());

    let indicator = if status.agent_count > 0 { "●" } else { "○" };

    format!(
        "{} {:<16} {} agents  {} pending",
        indicator, name, status.agent_count, status.pending_count
    )
}

/// Truncate tool input to a compact summary string.
fn truncate_tool_input(req: &DecisionRequest) -> String {
    // Show the most relevant field from the tool input
    let summary = if let Some(cmd) = req.tool_input.get("command").and_then(|v| v.as_str()) {
        cmd.to_string()
    } else if let Some(path) = req.tool_input.get("file_path").and_then(|v| v.as_str()) {
        path.to_string()
    } else if let Some(pattern) = req.tool_input.get("pattern").and_then(|v| v.as_str()) {
        pattern.to_string()
    } else {
        req.tool_input.to_string()
    };

    if summary.len() > 50 {
        format!("{}...", &summary[..47])
    } else {
        summary
    }
}
