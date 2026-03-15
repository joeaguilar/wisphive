use std::collections::HashMap;

use tokio::sync::{broadcast, oneshot};
use tracing::{info, warn};
use uuid::Uuid;
use wisphive_protocol::{Decision, DecisionFilter, DecisionRequest, ServerMessage};

/// The decision queue: holds pending tool-call decisions awaiting human response.
///
/// When a hook submits a DecisionRequest, it gets a oneshot receiver to block on.
/// When the TUI approves/denies, the oneshot sender fires and the hook unblocks.
pub struct DecisionQueue {
    /// Pending decisions awaiting human response. Maps request ID → oneshot sender.
    pending_senders: HashMap<Uuid, oneshot::Sender<Decision>>,
    /// Ordered list of pending requests (for TUI display).
    pending_items: Vec<DecisionRequest>,
    /// Broadcast channel to push events to all connected TUI clients.
    tui_tx: broadcast::Sender<ServerMessage>,
}

impl DecisionQueue {
    pub fn new(tui_tx: broadcast::Sender<ServerMessage>) -> Self {
        Self {
            pending_senders: HashMap::new(),
            pending_items: Vec::new(),
            tui_tx,
        }
    }

    /// Enqueue a decision request. Returns a oneshot receiver that the hook handler
    /// should await — it will resolve when the TUI sends approve/deny.
    pub fn enqueue(&mut self, req: DecisionRequest) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();

        info!(
            id = %req.id,
            agent = %req.agent_id,
            tool = %req.tool_name,
            project = %req.project.display(),
            "decision queued"
        );

        self.pending_senders.insert(req.id, tx);
        self.pending_items.push(req.clone());

        // Notify all connected TUIs
        let _ = self.tui_tx.send(ServerMessage::NewDecision(req));

        rx
    }

    /// Resolve a pending decision (approve or deny). Returns true if found.
    pub fn resolve(&mut self, id: Uuid, decision: Decision) -> bool {
        if let Some(tx) = self.pending_senders.remove(&id) {
            self.pending_items.retain(|r| r.id != id);

            info!(%id, ?decision, "decision resolved");

            let _ = self
                .tui_tx
                .send(ServerMessage::DecisionResolved { id, decision });

            // Send the decision to the waiting hook. If the hook already disconnected
            // (timed out), this just drops silently — that's fine.
            let _ = tx.send(decision);
            true
        } else {
            warn!(%id, "attempted to resolve unknown decision");
            false
        }
    }

    /// Resolve all pending decisions matching an optional filter.
    /// Returns the number of decisions resolved.
    pub fn resolve_all(&mut self, filter: &Option<DecisionFilter>, decision: Decision) -> usize {
        let ids: Vec<Uuid> = self
            .pending_items
            .iter()
            .filter(|req| filter.as_ref().map_or(true, |f| f.matches(req)))
            .map(|req| req.id)
            .collect();

        let count = ids.len();
        for id in ids {
            self.resolve(id, decision);
        }
        count
    }

    /// Get a snapshot of all pending items (for TUI initial sync).
    pub fn snapshot(&self) -> Vec<DecisionRequest> {
        self.pending_items.clone()
    }

    /// Number of pending decisions.
    pub fn len(&self) -> usize {
        self.pending_items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending_items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wisphive_protocol::AgentType;

    fn make_request(tool_name: &str, agent_id: &str, project: &str) -> DecisionRequest {
        DecisionRequest {
            id: Uuid::new_v4(),
            agent_id: agent_id.into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from(project),
            tool_name: tool_name.into(),
            tool_input: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        }
    }

    fn make_queue() -> DecisionQueue {
        let (tx, _) = broadcast::channel(64);
        DecisionQueue::new(tx)
    }

    // ════════════════════════════════════════════════════════════
    // Enqueue
    // ════════════════════════════════════════════════════════════

    #[test]
    fn enqueue_single_item() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let _rx = q.enqueue(req.clone());

        assert_eq!(q.len(), 1);
        assert!(!q.is_empty());
        let snap = q.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].tool_name, "Bash");
    }

    #[test]
    fn enqueue_multiple_preserves_order() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let r3 = make_request("Edit", "cc-1", "/muse");

        let _rx1 = q.enqueue(r1);
        let _rx2 = q.enqueue(r2);
        let _rx3 = q.enqueue(r3);

        assert_eq!(q.len(), 3);
        let snap = q.snapshot();
        assert_eq!(snap[0].tool_name, "Bash");
        assert_eq!(snap[1].tool_name, "Write");
        assert_eq!(snap[2].tool_name, "Edit");
    }

    // ════════════════════════════════════════════════════════════
    // Resolve
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn resolve_sends_decision_to_receiver() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req);

        assert!(q.resolve(id, Decision::Approve));
        let decision = rx.await.unwrap();
        assert_eq!(decision, Decision::Approve);
        assert_eq!(q.len(), 0);
    }

    #[tokio::test]
    async fn resolve_deny_sends_deny() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req);

        assert!(q.resolve(id, Decision::Deny));
        let decision = rx.await.unwrap();
        assert_eq!(decision, Decision::Deny);
    }

    #[test]
    fn resolve_unknown_id_returns_false() {
        let mut q = make_queue();
        let unknown_id = Uuid::new_v4();
        assert!(!q.resolve(unknown_id, Decision::Approve));
    }

    #[test]
    fn resolve_removes_from_snapshot() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let id1 = r1.id;

        let _rx1 = q.enqueue(r1);
        let _rx2 = q.enqueue(r2);

        q.resolve(id1, Decision::Approve);

        assert_eq!(q.len(), 1);
        let snap = q.snapshot();
        assert_eq!(snap[0].tool_name, "Write");
    }

    #[test]
    fn resolve_same_id_twice_returns_false_second_time() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let _rx = q.enqueue(req);

        assert!(q.resolve(id, Decision::Approve));
        assert!(!q.resolve(id, Decision::Approve));
    }

    #[test]
    fn resolve_does_not_panic_if_receiver_dropped() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req);

        // Drop the receiver (simulates hook disconnecting/timing out)
        drop(rx);

        // Should not panic — the send just silently fails
        assert!(q.resolve(id, Decision::Approve));
        assert_eq!(q.len(), 0);
    }

    // ════════════════════════════════════════════════════════════
    // Resolve all
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn resolve_all_no_filter_resolves_everything() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let r3 = make_request("Edit", "cc-3", "/retro");

        let rx1 = q.enqueue(r1);
        let rx2 = q.enqueue(r2);
        let rx3 = q.enqueue(r3);

        let count = q.resolve_all(&None, Decision::Approve);
        assert_eq!(count, 3);
        assert_eq!(q.len(), 0);

        assert_eq!(rx1.await.unwrap(), Decision::Approve);
        assert_eq!(rx2.await.unwrap(), Decision::Approve);
        assert_eq!(rx3.await.unwrap(), Decision::Approve);
    }

    #[tokio::test]
    async fn resolve_all_with_tool_filter() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let r3 = make_request("Bash", "cc-3", "/retro");

        let rx1 = q.enqueue(r1);
        let _rx2 = q.enqueue(r2);
        let rx3 = q.enqueue(r3);

        let filter = Some(DecisionFilter {
            tool_name: Some("Bash".into()),
            ..Default::default()
        });
        let count = q.resolve_all(&filter, Decision::Deny);

        assert_eq!(count, 2);
        assert_eq!(q.len(), 1); // Only Write remains
        assert_eq!(q.snapshot()[0].tool_name, "Write");

        assert_eq!(rx1.await.unwrap(), Decision::Deny);
        assert_eq!(rx3.await.unwrap(), Decision::Deny);
    }

    #[tokio::test]
    async fn resolve_all_with_project_filter() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/muse");
        let r3 = make_request("Edit", "cc-3", "/rpg");

        let rx1 = q.enqueue(r1);
        let rx2 = q.enqueue(r2);
        let _rx3 = q.enqueue(r3);

        let filter = Some(DecisionFilter {
            project: Some(PathBuf::from("/muse")),
            ..Default::default()
        });
        let count = q.resolve_all(&filter, Decision::Approve);

        assert_eq!(count, 2);
        assert_eq!(q.len(), 1);
        assert_eq!(q.snapshot()[0].project, PathBuf::from("/rpg"));

        assert_eq!(rx1.await.unwrap(), Decision::Approve);
        assert_eq!(rx2.await.unwrap(), Decision::Approve);
    }

    #[test]
    fn resolve_all_with_no_matches_returns_zero() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let _rx = q.enqueue(r1);

        let filter = Some(DecisionFilter {
            tool_name: Some("NonExistent".into()),
            ..Default::default()
        });
        let count = q.resolve_all(&filter, Decision::Approve);

        assert_eq!(count, 0);
        assert_eq!(q.len(), 1); // Nothing resolved
    }

    #[test]
    fn resolve_all_on_empty_queue() {
        let mut q = make_queue();
        let count = q.resolve_all(&None, Decision::Approve);
        assert_eq!(count, 0);
    }

    // ════════════════════════════════════════════════════════════
    // Broadcast to TUI
    // ════════════════════════════════════════════════════════════

    #[test]
    fn enqueue_broadcasts_new_decision() {
        let (tx, _) = broadcast::channel(64);
        let mut rx = tx.subscribe();
        let mut q = DecisionQueue::new(tx);

        let req = make_request("Bash", "cc-1", "/muse");
        let _hook_rx = q.enqueue(req.clone());

        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::NewDecision(r) => {
                assert_eq!(r.tool_name, "Bash");
                assert_eq!(r.agent_id, "cc-1");
            }
            _ => panic!("expected NewDecision"),
        }
    }

    #[test]
    fn resolve_broadcasts_decision_resolved() {
        let (tx, _) = broadcast::channel(64);
        let mut rx = tx.subscribe();
        let mut q = DecisionQueue::new(tx);

        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let _hook_rx = q.enqueue(req);

        // Skip the NewDecision broadcast
        let _ = rx.try_recv();

        q.resolve(id, Decision::Deny);

        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::DecisionResolved { id: rid, decision } => {
                assert_eq!(rid, id);
                assert_eq!(decision, Decision::Deny);
            }
            _ => panic!("expected DecisionResolved"),
        }
    }
}
