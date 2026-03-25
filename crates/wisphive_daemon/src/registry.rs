use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use tracing::info;
use wisphive_protocol::{AgentInfo, AgentType};

/// Internal entry wrapping AgentInfo.
struct AgentEntry {
    info: AgentInfo,
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
            entry.info.last_seen = now;
            (entry.info.clone(), false)
        } else {
            let info = AgentInfo {
                agent_id: agent_id.clone(),
                agent_type,
                project,
                connected_at: now,
                last_seen: now,
            };
            info!(agent_id = %info.agent_id, agent_type = %info.agent_type, "agent registered");
            self.agents.insert(
                agent_id,
                AgentEntry {
                    info: info.clone(),
                },
            );
            (info, true)
        }
    }

    /// Update last_seen for an agent. No-op if not registered.
    pub fn touch(&mut self, agent_id: &str) {
        if let Some(entry) = self.agents.get_mut(agent_id) {
            entry.info.last_seen = Utc::now();
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
            .filter(|(_, entry)| entry.info.last_seen < cutoff)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use wisphive_protocol::AgentType;

    fn test_project() -> PathBuf {
        PathBuf::from("/tmp/test-project")
    }

    // --- Basic operations ---

    #[test]
    fn new_registry_is_empty() {
        let reg = AgentRegistry::new();
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn register_new_agent_returns_is_new_true() {
        let mut reg = AgentRegistry::new();
        let (info, is_new) =
            reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        assert!(is_new);
        assert_eq!(info.agent_id, "agent-1");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_same_agent_twice_returns_is_new_false() {
        let mut reg = AgentRegistry::new();
        let (_info, is_new) =
            reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        assert!(is_new);

        let (_info2, is_new2) =
            reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        assert!(!is_new2);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_updates_last_seen_on_re_register() {
        let mut reg = AgentRegistry::new();
        let (first_info, _) =
            reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        let first_last_seen = first_info.last_seen;

        // Small sleep so the clock advances
        thread::sleep(Duration::from_millis(2));

        let (second_info, is_new) =
            reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        assert!(!is_new);
        assert!(second_info.last_seen >= first_last_seen);
    }

    #[test]
    fn register_different_agents() {
        let mut reg = AgentRegistry::new();
        reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        reg.register("agent-2".into(), AgentType::Red, test_project());
        reg.register(
            "agent-3".into(),
            AgentType::LocalLlm,
            PathBuf::from("/tmp/other"),
        );
        assert_eq!(reg.len(), 3);
    }

    // --- Get / list ---

    #[test]
    fn get_existing_agent() {
        let mut reg = AgentRegistry::new();
        let project = PathBuf::from("/home/user/myproject");
        reg.register("agent-x".into(), AgentType::Red, project.clone());

        let info = reg.get("agent-x").expect("agent should exist");
        assert_eq!(info.agent_id, "agent-x");
        assert_eq!(info.agent_type, AgentType::Red);
        assert_eq!(info.project, project);
    }

    #[test]
    fn get_nonexistent_agent() {
        let reg = AgentRegistry::new();
        assert!(reg.get("no-such-agent").is_none());
    }

    #[test]
    fn list_returns_all_agents() {
        let mut reg = AgentRegistry::new();
        reg.register("a".into(), AgentType::ClaudeCode, test_project());
        reg.register("b".into(), AgentType::Red, test_project());
        assert_eq!(reg.list().len(), 2);
    }

    // --- Touch ---

    #[test]
    fn touch_updates_last_seen() {
        let mut reg = AgentRegistry::new();
        reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        let before = reg.get("agent-1").unwrap().last_seen;

        thread::sleep(Duration::from_millis(2));
        reg.touch("agent-1");

        let after = reg.get("agent-1").unwrap().last_seen;
        assert!(after >= before);
    }

    #[test]
    fn touch_nonexistent_is_noop() {
        let mut reg = AgentRegistry::new();
        // Should not panic
        reg.touch("ghost-agent");
        assert!(reg.is_empty());
    }

    // --- Deregister ---

    #[test]
    fn deregister_existing_returns_info() {
        let mut reg = AgentRegistry::new();
        reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());

        let info = reg.deregister("agent-1").expect("should return info");
        assert_eq!(info.agent_id, "agent-1");
    }

    #[test]
    fn deregister_nonexistent_returns_none() {
        let mut reg = AgentRegistry::new();
        assert!(reg.deregister("nope").is_none());
    }

    #[test]
    fn deregister_removes_from_registry() {
        let mut reg = AgentRegistry::new();
        reg.register("agent-1".into(), AgentType::ClaudeCode, test_project());
        assert_eq!(reg.len(), 1);

        reg.deregister("agent-1");
        assert!(reg.get("agent-1").is_none());
        assert_eq!(reg.len(), 0);
        assert!(reg.is_empty());
    }

    // --- Reap ---

    #[test]
    fn reap_removes_inactive_agents() {
        let mut reg = AgentRegistry::new();
        reg.register("old-agent".into(), AgentType::ClaudeCode, test_project());

        // Duration::ZERO means cutoff = now, so any agent with last_seen < now is reaped
        thread::sleep(Duration::from_millis(2));
        let removed = reg.reap_inactive(Duration::ZERO);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], "old-agent");
        assert!(reg.is_empty());
    }

    #[test]
    fn reap_keeps_active_agents() {
        let mut reg = AgentRegistry::new();
        reg.register("fresh-agent".into(), AgentType::ClaudeCode, test_project());

        // Large timeout means cutoff is far in the past — agent is "active"
        let removed = reg.reap_inactive(Duration::from_secs(3600));
        assert!(removed.is_empty());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn reap_returns_removed_ids() {
        let mut reg = AgentRegistry::new();
        reg.register("a".into(), AgentType::ClaudeCode, test_project());
        reg.register("b".into(), AgentType::Red, test_project());
        reg.register("c".into(), AgentType::LocalLlm, test_project());

        thread::sleep(Duration::from_millis(2));
        let mut removed = reg.reap_inactive(Duration::ZERO);
        removed.sort();

        assert_eq!(removed, vec!["a", "b", "c"]);
        assert!(reg.is_empty());
    }

    #[test]
    fn reap_empty_registry() {
        let mut reg = AgentRegistry::new();
        let removed = reg.reap_inactive(Duration::ZERO);
        assert!(removed.is_empty());
    }

    // --- Default ---

    #[test]
    fn default_is_empty() {
        let reg = AgentRegistry::default();
        assert!(reg.is_empty());
    }
}
