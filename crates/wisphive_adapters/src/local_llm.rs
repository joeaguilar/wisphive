use anyhow::Result;
use async_trait::async_trait;
use tracing::info;
use wisphive_protocol::{AgentType, Decision};

use crate::adapter::{AdapterEvent, AgentAdapter};

/// Local LLM adapter (Ollama, llama.cpp, etc.).
///
/// For local LLMs, Wisphive IS the interface layer — there's no separate
/// agent to hook into. This adapter will:
/// - Communicate directly with Ollama's HTTP API
/// - Manage prompt/response cycles for simple tasks
/// - Report tool calls (if the LLM uses tool-calling features) through
///   the decision queue
///
/// Status: STUB — implementation comes post-MVP.
pub struct LocalLlmAdapter {
    _event_tx: Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
    _base_url: String,
}

impl LocalLlmAdapter {
    pub fn new(base_url: &str) -> Self {
        Self {
            _event_tx: None,
            _base_url: base_url.to_string(),
        }
    }
}

impl Default for LocalLlmAdapter {
    fn default() -> Self {
        Self::new("http://localhost:11434")
    }
}

#[async_trait]
impl AgentAdapter for LocalLlmAdapter {
    fn agent_type(&self) -> AgentType {
        AgentType::LocalLlm
    }

    fn name(&self) -> &str {
        "Local LLM"
    }

    async fn start(&mut self, event_tx: tokio::sync::mpsc::Sender<AdapterEvent>) -> Result<()> {
        info!(url = %self._base_url, "Local LLM adapter started (stub — HTTP integration pending)");
        self._event_tx = Some(event_tx);
        // TODO: Implement Ollama HTTP API client
        Ok(())
    }

    async fn respond(&self, _agent_id: &str, _decision: Decision) -> Result<()> {
        // TODO: Forward decision to the LLM session
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Local LLM adapter stopped");
        self._event_tx = None;
        Ok(())
    }
}
