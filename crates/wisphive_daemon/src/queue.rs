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
