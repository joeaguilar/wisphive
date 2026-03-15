use anyhow::Result;
use async_trait::async_trait;
use tracing::info;
use wisphive_protocol::{AgentType, Decision};

use crate::adapter::{AdapterEvent, AgentAdapter};

/// Claude Code adapter.
///
/// Claude Code integration works through hooks: the wisphive-hook binary
/// runs on every tool call and connects to the daemon's socket directly.
/// This adapter is primarily a bookkeeping wrapper — the actual hook
/// connection handling happens in the daemon's server module.
///
/// In the future, this adapter could also:
/// - Launch Claude Code instances (`claude --print`)
/// - Monitor running Claude Code processes
/// - Manage hook installation across projects
pub struct ClaudeCodeAdapter {
    _event_tx: Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self { _event_tx: None }
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn agent_type(&self) -> AgentType {
        AgentType::ClaudeCode
    }

    fn name(&self) -> &str {
        "Claude Code"
    }

    async fn start(&mut self, event_tx: tokio::sync::mpsc::Sender<AdapterEvent>) -> Result<()> {
        info!("Claude Code adapter started (hook-based, passive)");
        self._event_tx = Some(event_tx);
        // No active work needed — hooks push events to the daemon directly.
        Ok(())
    }

    async fn respond(&self, _agent_id: &str, _decision: Decision) -> Result<()> {
        // Responses are sent directly through the hook's socket connection
        // by the daemon's server module. This is a no-op for Claude Code.
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Claude Code adapter stopped");
        self._event_tx = None;
        Ok(())
    }
}
