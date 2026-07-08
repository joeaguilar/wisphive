//! Per-requester rate limiting for terminal replay (itr#98).
//!
//! `TermReplay` streams a session's complete byte history — including
//! anything typed into it (sudo passwords, pasted keys) — so a client that
//! can issue it must not be able to scrape every session's history in a
//! tight loop. This limiter is deliberately modelled on
//! [`crate::sudo_gate::ReauthRegistry`]: a small in-memory
//! `Arc<Mutex<HashMap>>` keyed by requester identity (the same
//! `resolver_label` string used for the audit trail), no external crates.
//!
//! Authorship and ACL checks live in `server.rs`; this module only bounds how
//! many replay attempts a requester can make in a short window. The limit
//! recovers by itself as the window slides.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Sliding window over which replay requests are counted.
pub const REPLAY_WINDOW: Duration = Duration::from_secs(60);

/// Maximum replay requests per requester within [`REPLAY_WINDOW`]. Generous
/// for interactive use (a human re-watching a handful of sessions) while
/// making bulk scraping of session history slow and loud — every attempt,
/// throttled or not, lands in the audit log.
pub const REPLAY_MAX_PER_WINDOW: usize = 10;

/// Per-requester sliding-window counter. Cheap to clone (Arc internally);
/// the server holds one instance and shares it across all connections so a
/// client cannot reset its budget by reconnecting.
#[derive(Clone)]
pub struct ReplayRateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    window: Duration,
    max_per_window: usize,
}

impl Default for ReplayRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayRateLimiter {
    /// Build a limiter with the default window and budget.
    pub fn new() -> Self {
        Self::with_limits(REPLAY_WINDOW, REPLAY_MAX_PER_WINDOW)
    }

    /// Build with custom limits. Exposed for tests that need to exercise
    /// expiry without sleeping through the real window.
    pub fn with_limits(window: Duration, max_per_window: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            window,
            max_per_window,
        }
    }

    /// Record an attempt for `key` and return whether it fits the budget.
    ///
    /// Denied attempts are not recorded, so hammering while throttled does
    /// not extend the throttle. Stale hits across all keys are pruned on
    /// each call, bounding the map without a background sweep task (same
    /// pattern as `ReauthRegistry::touch`).
    pub async fn try_acquire(&self, key: &str) -> bool {
        let mut map = self.inner.lock().await;
        let now = Instant::now();
        map.retain(|_, hits| {
            hits.retain(|t| now.duration_since(*t) < self.window);
            !hits.is_empty()
        });
        let hits = map.entry(key.to_string()).or_default();
        if hits.len() >= self.max_per_window {
            return false;
        }
        hits.push(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[tokio::test]
    async fn allows_up_to_budget_then_denies() {
        let gate = ReplayRateLimiter::with_limits(Duration::from_secs(60), 3);
        for _ in 0..3 {
            assert!(gate.try_acquire("human:tui").await);
        }
        assert!(!gate.try_acquire("human:tui").await);
    }

    #[tokio::test]
    async fn budgets_are_per_requester() {
        let gate = ReplayRateLimiter::with_limits(Duration::from_secs(60), 1);
        assert!(gate.try_acquire("human:web:a").await);
        assert!(!gate.try_acquire("human:web:a").await);
        assert!(
            gate.try_acquire("human:web:b").await,
            "one requester's burst must not throttle another"
        );
    }

    #[tokio::test]
    async fn window_slides() {
        let gate = ReplayRateLimiter::with_limits(Duration::from_millis(20), 1);
        assert!(gate.try_acquire("k").await);
        assert!(!gate.try_acquire("k").await);
        sleep(Duration::from_millis(40));
        assert!(
            gate.try_acquire("k").await,
            "budget must recover once the window slides past the old hits"
        );
    }

    #[tokio::test]
    async fn denied_attempts_do_not_extend_the_throttle() {
        let gate = ReplayRateLimiter::with_limits(Duration::from_millis(30), 1);
        assert!(gate.try_acquire("k").await);
        // Hammer while throttled — these must not push the window forward.
        for _ in 0..5 {
            assert!(!gate.try_acquire("k").await);
        }
        sleep(Duration::from_millis(60));
        assert!(gate.try_acquire("k").await);
    }

    #[tokio::test]
    async fn clones_share_state() {
        let a = ReplayRateLimiter::with_limits(Duration::from_secs(60), 1);
        let b = a.clone();
        assert!(a.try_acquire("k").await);
        assert!(
            !b.try_acquire("k").await,
            "clone must see the same budget (reconnect must not reset it)"
        );
    }
}
