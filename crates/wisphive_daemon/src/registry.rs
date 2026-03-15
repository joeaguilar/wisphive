use std::collections::HashMap;

use chrono::Utc;
use tracing::info;
use wisphive_protocol::{AgentInfo, AgentType};

/// Tracks connected agent instances.
pub struct AgentRegistry {
    agents: HashMap<String, AgentInfo>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register a new agent connection. Returns the AgentInfo created.
    pub fn register(
        &mut self,
        agent_id: String,
        agent_type: AgentType,
        project: std::path::PathBuf,
    ) -> AgentInfo {
        let info = AgentInfo {
            agent_id: agent_id.clone(),
            agent_type,
            project,
            started_at: Utc::now(),
        };
        info!(agent_id = %info.agent_id, agent_type = %info.agent_type, "agent registered");
        self.agents.insert(agent_id, info.clone());
        info
    }

    /// Remove an agent from the registry.
    pub fn deregister(&mut self, agent_id: &str) -> Option<AgentInfo> {
        let removed = self.agents.remove(agent_id);
        if let Some(ref info) = removed {
            info!(agent_id = %info.agent_id, "agent deregistered");
        }
        removed
    }

    /// Get info for a specific agent.
    pub fn get(&self, agent_id: &str) -> Option<&AgentInfo> {
        self.agents.get(agent_id)
    }

    /// List all connected agents.
    pub fn list(&self) -> Vec<&AgentInfo> {
        self.agents.values().collect()
    }

    /// Number of connected agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
