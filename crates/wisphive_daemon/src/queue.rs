use std::collections::{HashMap, HashSet};

use tokio::sync::{broadcast, oneshot};
use tracing::{info, warn};
use uuid::Uuid;
use wisphive_protocol::{Decision, DecisionFilter, DecisionRequest, RichDecision, ServerMessage};

use crate::notify::sanitize_for_log;

/// The decision queue: holds pending tool-call decisions awaiting human response.
///
/// When a hook submits a DecisionRequest, it gets a oneshot receiver to block on.
/// When the TUI approves/denies, the oneshot sender fires and the hook unblocks.
pub struct DecisionQueue {
    /// Pending decisions awaiting human response. Maps request ID → oneshot sender.
    pending_senders: HashMap<Uuid, oneshot::Sender<RichDecision>>,
    /// Ordered list of pending requests (for TUI display).
    pending_items: Vec<DecisionRequest>,
    /// Requests claimed for durable resolution but not released yet. Keeping
    /// them accounted here prevents the SpawnAgent cap from being bypassed by
    /// many concurrent slow persistence operations.
    claimed_items: HashMap<Uuid, DecisionRequest>,
    /// Server-side provenance for synthetic managed-spawn decisions. Wire
    /// fields are attacker-controlled, so identity cannot be inferred from a
    /// tool/agent string supplied by a hook.
    managed_spawn_ids: HashSet<Uuid>,
    /// Broadcast channel to push events to all connected TUI clients.
    tui_tx: broadcast::Sender<ServerMessage>,
}

/// A decision removed from the live queue but not yet released to its waiting
/// worker. Spawn decisions use this two-phase path so their audit write can
/// complete before an approval is capable of launching a process.
pub struct ClaimedDecision {
    request: DecisionRequest,
    sender: oneshot::Sender<RichDecision>,
}

impl ClaimedDecision {
    pub fn request(&self) -> &DecisionRequest {
        &self.request
    }
}

impl DecisionQueue {
    pub fn new(tui_tx: broadcast::Sender<ServerMessage>) -> Self {
        Self {
            pending_senders: HashMap::new(),
            pending_items: Vec::new(),
            claimed_items: HashMap::new(),
            managed_spawn_ids: HashSet::new(),
            tui_tx,
        }
    }

    /// Enqueue a decision request. Returns a oneshot receiver that the hook handler
    /// should await — it will resolve when the TUI sends approve/deny.
    ///
    /// Returns `None` if a request with the same id is already pending: the id
    /// is chosen by the hook (attacker-influenced over the local socket), and
    /// silently overwriting would drop the victim's oneshot sender — an
    /// instant fail-open approve — and leave two items sharing one sender
    /// (itr#370). The caller must reject the duplicate.
    pub fn enqueue(&mut self, req: DecisionRequest) -> Option<oneshot::Receiver<RichDecision>> {
        self.enqueue_inner(req, false)
    }

    pub fn enqueue_managed_spawn(
        &mut self,
        req: DecisionRequest,
    ) -> Option<oneshot::Receiver<RichDecision>> {
        self.enqueue_inner(req, true)
    }

    fn enqueue_inner(
        &mut self,
        req: DecisionRequest,
        managed_spawn: bool,
    ) -> Option<oneshot::Receiver<RichDecision>> {
        if self.pending_senders.contains_key(&req.id)
            || self.claimed_items.contains_key(&req.id)
            || self.managed_spawn_ids.contains(&req.id)
        {
            warn!(
                id = %req.id,
                agent = %sanitize_for_log(&req.agent_id),
                tool = %sanitize_for_log(&req.tool_name),
                "rejected duplicate decision id (itr#370)"
            );
            return None;
        }

        let (tx, rx) = oneshot::channel();

        info!(
            id = %req.id,
            agent = %sanitize_for_log(&req.agent_id),
            tool = %sanitize_for_log(&req.tool_name),
            project = %sanitize_for_log(&req.project.to_string_lossy()),
            "decision queued"
        );

        self.pending_senders.insert(req.id, tx);
        self.pending_items.push(req.clone());
        if managed_spawn {
            self.managed_spawn_ids.insert(req.id);
        }

        // Notify all connected TUIs
        let _ = self.tui_tx.send(ServerMessage::NewDecision(req));

        Some(rx)
    }

    /// Remove a pending decision that was resolved OUTSIDE the queue (hook
    /// timeout, channel drop, or hook disconnect) and broadcast the outcome so
    /// TUI/web state stays consistent with the audit log (itr#363). Unlike
    /// [`Self::resolve`], nothing is sent to the hook — the caller already
    /// answered it (or it is gone).
    pub fn finalize_local(&mut self, id: Uuid, decision: Decision) -> bool {
        let had_sender = self.pending_senders.remove(&id).is_some();
        let before = self.pending_items.len();
        self.pending_items.retain(|r| r.id != id);
        let had_claim = self.claimed_items.remove(&id).is_some();
        self.managed_spawn_ids.remove(&id);
        let removed = had_sender || had_claim || self.pending_items.len() != before;
        if removed {
            info!(%id, ?decision, "decision finalized outside the queue");
            let _ = self
                .tui_tx
                .send(ServerMessage::DecisionResolved { id, decision });
        }
        removed
    }

    /// Atomically remove a decision and its sender without resolving it yet.
    /// The caller owns the returned claim and must call [`Self::complete_claim`]
    /// after durable persistence/cleanup. Dropping a claim fails closed because
    /// it drops the oneshot sender without sending an approval.
    pub fn claim(&mut self, id: Uuid) -> Option<ClaimedDecision> {
        let position = self.pending_items.iter().position(|req| req.id == id)?;
        let sender = self.pending_senders.remove(&id)?;
        let request = self.pending_items.remove(position);
        self.claimed_items.insert(id, request.clone());
        info!(
            %id,
            tool = %sanitize_for_log(&request.tool_name),
            "decision claimed for durable resolution"
        );
        Some(ClaimedDecision { request, sender })
    }

    /// Release a previously claimed decision after its persistence owner has
    /// completed, removing it from the in-flight accounting at the same time.
    pub fn complete_claim(&mut self, claim: ClaimedDecision, rich: RichDecision) -> bool {
        let id = claim.request.id;
        if self.claimed_items.remove(&id).is_none() {
            warn!(%id, "claimed decision was already finalized");
            return false;
        }
        self.managed_spawn_ids.remove(&id);
        info!(%id, decision = ?rich.decision, "claimed decision completed");
        let _ = self.tui_tx.send(ServerMessage::DecisionResolved {
            id,
            decision: rich.decision,
        });
        claim.sender.send(rich).is_ok()
    }

    /// Resolve a pending decision with a rich response. Returns true if found.
    pub fn resolve(&mut self, id: Uuid, rich: RichDecision) -> bool {
        if self.managed_spawn_ids.contains(&id) {
            warn!(%id, "generic resolve refused managed SpawnAgent decision");
            return false;
        }
        if let Some(tx) = self.pending_senders.remove(&id) {
            self.pending_items.retain(|r| r.id != id);
            self.managed_spawn_ids.remove(&id);

            info!(%id, decision = ?rich.decision, "decision resolved");

            let _ = self.tui_tx.send(ServerMessage::DecisionResolved {
                id,
                decision: rich.decision,
            });

            // Send the rich decision to the waiting hook. If the hook already disconnected
            // (timed out), this just drops silently — that's fine.
            let _ = tx.send(rich);
            true
        } else {
            warn!(%id, "attempted to resolve unknown decision");
            false
        }
    }

    /// Resolve all pending decisions matching an optional filter.
    /// Returns the IDs of resolved decisions. `resolver` identifies the
    /// resolving client for the audit trail (itr#88).
    pub fn resolve_all(
        &mut self,
        filter: &Option<DecisionFilter>,
        decision: Decision,
        resolver: Option<&str>,
    ) -> Vec<Uuid> {
        let ids: Vec<Uuid> = self
            .pending_items
            .iter()
            .filter(|req| filter.as_ref().is_none_or(|f| f.matches(req)))
            // A managed process must never be released through the generic
            // bulk path for any outcome. Its two-phase finalizer is the sole
            // persistence/release owner (itr#94).
            .filter(|req| !self.managed_spawn_ids.contains(&req.id))
            .map(|req| req.id)
            .collect();

        for id in &ids {
            let rich = RichDecision {
                resolver: resolver.map(str::to_string),
                ..RichDecision::from(decision)
            };
            self.resolve(*id, rich);
        }
        ids
    }

    /// Get a snapshot of all pending items (for TUI initial sync).
    pub fn snapshot(&self) -> Vec<DecisionRequest> {
        let mut snapshot = self.pending_items.clone();
        snapshot.extend(self.claimed_items.values().cloned());
        snapshot
    }

    /// Look up a pending request by id without removing it. Used by the
    /// sudo gate, which needs to see a decision's tool_name before deciding
    /// whether to let an approve through — if the decision has already been
    /// resolved or never existed, this returns `None` and the caller falls
    /// back to `resolve`'s "unknown decision" path.
    pub fn peek(&self, id: Uuid) -> Option<&DecisionRequest> {
        self.pending_items.iter().find(|r| r.id == id)
    }

    /// Count queued items for one tool. Used to enforce small per-class caps
    /// while holding the queue lock, so concurrent connections cannot race the
    /// check and enqueue steps.
    pub fn count_tool(&self, tool_name: &str) -> usize {
        self.pending_items
            .iter()
            .chain(self.claimed_items.values())
            .filter(|req| req.tool_name == tool_name)
            .count()
    }

    pub fn managed_spawn_count(&self) -> usize {
        self.managed_spawn_ids.len()
    }

    pub fn is_managed_spawn(&self, id: Uuid) -> bool {
        self.managed_spawn_ids.contains(&id)
    }

    /// Number of pending decisions.
    pub fn len(&self) -> usize {
        self.pending_items.len() + self.claimed_items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending_items.is_empty() && self.claimed_items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wisphive_protocol::QUEUE_MAKE_REQUEST as make_request;

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

    #[tokio::test]
    async fn duplicate_id_is_rejected_and_victim_survives() {
        // itr#370: a second request reusing a pending id must be rejected —
        // overwriting would drop the victim's sender (instant fail-open).
        let mut q = make_queue();
        let victim = make_request("Bash", "cc-victim", "/muse");
        let id = victim.id;
        let victim_rx = q.enqueue(victim).unwrap();

        let mut attacker = make_request("Write", "cc-attacker", "/evil");
        attacker.id = id;
        assert!(
            q.enqueue(attacker).is_none(),
            "duplicate id must be rejected"
        );

        // The victim's entry is intact and still resolvable.
        assert_eq!(q.len(), 1);
        assert_eq!(q.snapshot()[0].agent_id, "cc-victim");
        assert!(q.resolve(id, RichDecision::approve()));
        assert_eq!(victim_rx.await.unwrap().decision, Decision::Approve);
    }

    #[tokio::test]
    async fn duplicate_id_is_rejected_while_victim_is_claimed() {
        let mut q = make_queue();
        let victim = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
        let id = victim.id;
        let victim_rx = q.enqueue(victim).unwrap();
        let claim = q.claim(id).unwrap();

        let mut attacker = make_request("Write", "cc-attacker", "/evil");
        attacker.id = id;
        assert!(
            q.enqueue(attacker).is_none(),
            "claimed IDs remain reserved until durable completion"
        );
        assert_eq!(q.len(), 1);
        assert!(q.complete_claim(claim, RichDecision::approve()));
        assert_eq!(victim_rx.await.unwrap().decision, Decision::Approve);
    }

    #[test]
    fn finalize_local_removes_entry_and_broadcasts() {
        // itr#363: a timeout/disconnect resolution outside the queue must
        // clear the pending entry and tell TUIs, so a later human resolve
        // can't produce a contradictory state.
        let (tx, _) = broadcast::channel(64);
        let mut rx = tx.subscribe();
        let mut q = DecisionQueue::new(tx);

        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let _hook_rx = q.enqueue(req);
        let _ = rx.try_recv(); // skip NewDecision

        assert!(q.finalize_local(id, Decision::Approve));
        assert_eq!(q.len(), 0, "entry removed from pending items");
        match rx.try_recv().unwrap() {
            ServerMessage::DecisionResolved { id: rid, decision } => {
                assert_eq!(rid, id);
                assert_eq!(decision, Decision::Approve);
            }
            other => panic!("expected DecisionResolved, got {other:?}"),
        }

        // A later human resolve finds nothing — no second broadcast, no lie.
        assert!(!q.resolve(id, RichDecision::deny()));
        assert!(!q.finalize_local(id, Decision::Deny));
        assert!(rx.try_recv().is_err(), "no broadcast for the stale resolve");
    }

    // ════════════════════════════════════════════════════════════
    // Resolve
    // ════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn resolve_sends_decision_to_receiver() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req).unwrap();

        assert!(q.resolve(id, RichDecision::approve()));
        let decision = rx.await.unwrap();
        assert_eq!(decision.decision, Decision::Approve);
        assert_eq!(q.len(), 0);
    }

    #[tokio::test]
    async fn resolve_deny_sends_deny() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req).unwrap();

        assert!(q.resolve(id, RichDecision::deny()));
        let decision = rx.await.unwrap();
        assert_eq!(decision.decision, Decision::Deny);
    }

    #[tokio::test]
    async fn claim_does_not_release_worker_until_completed() {
        let mut q = make_queue();
        let req = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
        let id = req.id;
        let mut rx = Box::pin(q.enqueue_managed_spawn(req).unwrap());

        let claimed = q.claim(id).expect("pending decision should be claimable");
        assert_eq!(claimed.request().tool_name, "SpawnAgent");
        assert_eq!(
            q.count_tool("SpawnAgent"),
            1,
            "claimed work remains inside the global pending cap"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), rx.as_mut())
                .await
                .is_err(),
            "claim alone must not release the waiting worker"
        );

        assert!(q.complete_claim(claimed, RichDecision::approve()));
        assert_eq!(q.count_tool("SpawnAgent"), 0);
        assert_eq!(rx.await.unwrap().decision, Decision::Approve);
    }

    #[test]
    fn resolve_unknown_id_returns_false() {
        let mut q = make_queue();
        let unknown_id = Uuid::new_v4();
        assert!(!q.resolve(unknown_id, RichDecision::approve()));
    }

    #[test]
    fn resolve_removes_from_snapshot() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let id1 = r1.id;

        let _rx1 = q.enqueue(r1);
        let _rx2 = q.enqueue(r2);

        q.resolve(id1, RichDecision::approve());

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

        assert!(q.resolve(id, RichDecision::approve()));
        assert!(!q.resolve(id, RichDecision::approve()));
    }

    #[test]
    fn resolve_does_not_panic_if_receiver_dropped() {
        let mut q = make_queue();
        let req = make_request("Bash", "cc-1", "/muse");
        let id = req.id;
        let rx = q.enqueue(req).unwrap();

        // Drop the receiver (simulates hook disconnecting/timing out)
        drop(rx);

        // Should not panic — the send just silently fails
        assert!(q.resolve(id, RichDecision::approve()));
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

        let rx1 = q.enqueue(r1).unwrap();
        let rx2 = q.enqueue(r2).unwrap();
        let rx3 = q.enqueue(r3).unwrap();

        let ids = q.resolve_all(&None, Decision::Approve, None);
        assert_eq!(ids.len(), 3);
        assert_eq!(q.len(), 0);

        assert_eq!(rx1.await.unwrap().decision, Decision::Approve);
        assert_eq!(rx2.await.unwrap().decision, Decision::Approve);
        assert_eq!(rx3.await.unwrap().decision, Decision::Approve);
    }

    #[tokio::test]
    async fn generic_bulk_resolution_always_skips_managed_spawn() {
        let mut q = make_queue();
        let spawn = make_request("SpawnAgent", "wisphive-daemon:spawn", "/muse");
        let spawn_id = spawn.id;
        let spawn_rx = q.enqueue_managed_spawn(spawn).unwrap();
        let bash = make_request("Bash", "cc-1", "/muse");
        let bash_rx = q.enqueue(bash).unwrap();

        let approved = q.resolve_all(&None, Decision::Approve, Some("human:tui"));
        assert_eq!(approved.len(), 1, "only the ordinary tool is bulk-approved");
        assert_eq!(bash_rx.await.unwrap().decision, Decision::Approve);
        assert_eq!(q.count_tool("SpawnAgent"), 1);

        let denied = q.resolve_all(&None, Decision::Deny, Some("human:tui"));
        assert!(denied.is_empty());
        assert_eq!(q.count_tool("SpawnAgent"), 1);
        assert!(!q.resolve(spawn_id, RichDecision::deny()));
        let claim = q.claim(spawn_id).unwrap();
        assert!(q.complete_claim(claim, RichDecision::deny()));
        assert_eq!(spawn_rx.await.unwrap().decision, Decision::Deny);
    }

    #[tokio::test]
    async fn resolve_all_with_tool_filter() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/rpg");
        let r3 = make_request("Bash", "cc-3", "/retro");

        let rx1 = q.enqueue(r1).unwrap();
        let _rx2 = q.enqueue(r2);
        let rx3 = q.enqueue(r3).unwrap();

        let filter = Some(DecisionFilter {
            tool_name: Some("Bash".into()),
            ..Default::default()
        });
        let ids = q.resolve_all(&filter, Decision::Deny, None);

        assert_eq!(ids.len(), 2);
        assert_eq!(q.len(), 1); // Only Write remains
        assert_eq!(q.snapshot()[0].tool_name, "Write");

        assert_eq!(rx1.await.unwrap().decision, Decision::Deny);
        assert_eq!(rx3.await.unwrap().decision, Decision::Deny);
    }

    #[tokio::test]
    async fn resolve_all_with_project_filter() {
        let mut q = make_queue();
        let r1 = make_request("Bash", "cc-1", "/muse");
        let r2 = make_request("Write", "cc-2", "/muse");
        let r3 = make_request("Edit", "cc-3", "/rpg");

        let rx1 = q.enqueue(r1).unwrap();
        let rx2 = q.enqueue(r2).unwrap();
        let _rx3 = q.enqueue(r3);

        let filter = Some(DecisionFilter {
            project: Some(PathBuf::from("/muse")),
            ..Default::default()
        });
        let ids = q.resolve_all(&filter, Decision::Approve, None);

        assert_eq!(ids.len(), 2);
        assert_eq!(q.len(), 1);
        assert_eq!(q.snapshot()[0].project, PathBuf::from("/rpg"));

        assert_eq!(rx1.await.unwrap().decision, Decision::Approve);
        assert_eq!(rx2.await.unwrap().decision, Decision::Approve);
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
        let ids = q.resolve_all(&filter, Decision::Approve, None);

        assert!(ids.is_empty());
        assert_eq!(q.len(), 1); // Nothing resolved
    }

    #[test]
    fn resolve_all_on_empty_queue() {
        let mut q = make_queue();
        let ids = q.resolve_all(&None, Decision::Approve, None);
        assert!(ids.is_empty());
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

        q.resolve(id, RichDecision::deny());

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
