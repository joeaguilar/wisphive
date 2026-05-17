use anyhow::Result;
use async_trait::async_trait;
use tracing::info;
use wisphive_protocol::{AgentType, Decision};

use crate::adapter::{AdapterEvent, AgentAdapter};

/// Codex adapter.
///
/// Codex integration works through hooks: the wisphive-hook binary runs on
/// supported Codex hook events and connects to the daemon's socket directly.
/// This adapter is a bookkeeping wrapper; hook connection handling happens in
/// the daemon's server module.
pub struct CodexAdapter {
    _event_tx: Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self { _event_tx: None }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn agent_type(&self) -> AgentType {
        AgentType::Codex
    }

    fn name(&self) -> &str {
        "Codex"
    }

    async fn start(&mut self, event_tx: tokio::sync::mpsc::Sender<AdapterEvent>) -> Result<()> {
        info!("Codex adapter started (hook-based, passive)");
        self._event_tx = Some(event_tx);
        Ok(())
    }

    async fn respond(&self, _agent_id: &str, _decision: Decision) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Codex adapter stopped");
        self._event_tx = None;
        Ok(())
    }
}
