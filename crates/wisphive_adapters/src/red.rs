use anyhow::Result;
use async_trait::async_trait;
use tracing::info;
use wisphive_protocol::{AgentType, Decision};

use crate::adapter::{AdapterEvent, AgentAdapter};

/// Red agent adapter.
///
/// Red supports --rpc mode, which exposes a bidirectional JSON interface
/// over stdin/stdout. This adapter will:
/// - Spawn Red instances as child processes in --rpc mode
/// - Forward tool call decisions through the RPC channel
/// - Monitor Red's output for events
///
/// Status: STUB — implementation comes post-MVP.
pub struct RedAdapter {
    _event_tx: Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
}

impl RedAdapter {
    pub fn new() -> Self {
        Self { _event_tx: None }
    }
}

impl Default for RedAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for RedAdapter {
    fn agent_type(&self) -> AgentType {
        AgentType::Red
    }

    fn name(&self) -> &str {
        "Red"
    }

    async fn start(&mut self, event_tx: tokio::sync::mpsc::Sender<AdapterEvent>) -> Result<()> {
        info!("Red adapter started (stub — RPC integration pending)");
        self._event_tx = Some(event_tx);
        // TODO: Implement Red RPC connection management
        Ok(())
    }

    async fn respond(&self, _agent_id: &str, _decision: Decision) -> Result<()> {
        // TODO: Forward decision through Red's RPC channel
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Red adapter stopped");
        self._event_tx = None;
        Ok(())
    }
}
