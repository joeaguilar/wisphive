use anyhow::Result;
use async_trait::async_trait;
use wisphive_protocol::{AgentType, Decision};

/// Trait for adapting different agent types to the Wisphive protocol.
///
/// Each agent type (Claude Code, Red, local LLMs) has its own communication
/// mechanism. Adapters translate between the agent's native protocol and
/// Wisphive's internal event system.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// The type of agent this adapter handles.
    fn agent_type(&self) -> AgentType;

    /// Human-readable name for this adapter.
    fn name(&self) -> &str;

    /// Start the adapter. This may involve listening for incoming connections
    /// (Claude Code hooks push events) or actively connecting to agents
    /// (Red RPC, Ollama HTTP).
    ///
    /// The `event_tx` sender should be used to push agent events into the
    /// Wisphive daemon's processing pipeline.
    async fn start(&mut self, event_tx: tokio::sync::mpsc::Sender<AdapterEvent>) -> Result<()>;

    /// Send a decision response back to the agent.
    async fn respond(&self, agent_id: &str, decision: Decision) -> Result<()>;

    /// Gracefully stop the adapter.
    async fn stop(&mut self) -> Result<()>;
}

/// Events produced by adapters for the daemon to process.
#[derive(Debug)]
pub enum AdapterEvent {
    /// A new agent has connected.
    AgentConnected {
        agent_id: String,
        agent_type: AgentType,
        project: std::path::PathBuf,
    },
    /// An agent has disconnected.
    AgentDisconnected { agent_id: String },
    /// An agent is requesting a decision on a tool call.
    DecisionNeeded {
        request: wisphive_protocol::DecisionRequest,
    },
}
