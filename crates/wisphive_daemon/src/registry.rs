use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::info;
use wisphive_protocol::{AgentInfo, AgentType};

/// Internal entry wrapping AgentInfo with daemon bookkeeping.
struct AgentEntry {
    info: AgentInfo,
    last_seen: DateTime<Utc>,
}

/// Tracks connected agent instances.
pub struct AgentRegistry {
    agents: HashMap<String, AgentEntry>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register an agent. Returns (AgentInfo, is_new).
    /// If already registered, updates last_seen and returns is_new=false.
    pub fn register(
        &mut self,
        agent_id: String,
        agent_type: AgentType,
        project: std::path::PathBuf,
    ) -> (AgentInfo, bool) {
        let now = Utc::now();
        if let Some(entry) = self.agents.get_mut(&agent_id) {
            entry.last_seen = now;
            (entry.info.clone(), false)
        } else {
            let info = AgentInfo {
                agent_id: agent_id.clone(),
                agent_type,
                project,
                started_at: now,
            };
            info!(agent_id = %info.agent_id, agent_type = %info.agent_type, "agent registered");
            self.agents.insert(
                agent_id,
                AgentEntry {
                    info: info.clone(),
                    last_seen: now,
                },
            );
            (info, true)
        }
    }

    /// Update last_seen for an agent. No-op if not registered.
    pub fn touch(&mut self, agent_id: &str) {
        if let Some(entry) = self.agents.get_mut(agent_id) {
            entry.last_seen = Utc::now();
        }
    }

    /// Remove an agent from the registry.
    pub fn deregister(&mut self, agent_id: &str) -> Option<AgentInfo> {
        let removed = self.agents.remove(agent_id);
        if let Some(ref entry) = removed {
            info!(agent_id = %entry.info.agent_id, "agent deregistered");
        }
        removed.map(|e| e.info)
    }

    /// Reap agents inactive longer than the given timeout.
    /// Returns the agent_ids that were removed.
    pub fn reap_inactive(&mut self, timeout: Duration) -> Vec<String> {
        let cutoff = Utc::now() - chrono::Duration::from_std(timeout).unwrap_or_default();
        let expired: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, entry)| entry.last_seen < cutoff)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &expired {
            info!(agent_id = %id, "agent reaped (inactive)");
            self.agents.remove(id);
        }

        expired
    }

    /// Snapshot of all currently registered agents.
    pub fn snapshot(&self) -> Vec<AgentInfo> {
        self.agents.values().map(|e| e.info.clone()).collect()
    }

    /// Get info for a specific agent.
    pub fn get(&self, agent_id: &str) -> Option<&AgentInfo> {
        self.agents.get(agent_id).map(|e| &e.info)
    }

    /// List all connected agents.
    pub fn list(&self) -> Vec<&AgentInfo> {
        self.agents.values().map(|e| &e.info).collect()
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
