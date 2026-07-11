//! Sudo-mode gate for web-device approvals.
//!
//! Certain tools (Bash, Write, Edit, NotebookEdit, MultiEdit, ConfigChange,
//! SpawnAgent)
//! are powerful enough that a stolen device token shouldn't be able to
//! approve them on its own — the operator must re-enter the account password
//! within a short window first. This mirrors `sudo`: the shell is
//! authenticated, but destructive operations get a second challenge.
//!
//! # Model
//!
//! - Each authenticated web device gets a freshness timestamp in
//!   [`ReauthRegistry`] after a successful `/api/auth/reauth` hits the
//!   daemon.
//! - When the daemon sees an Approve/ApproveAll from a device whose envelope
//!   carries a `device_id`, it looks up the target decision's tool. If the
//!   tool is in [`SUDO_TOOLS`] and the device's timestamp is older than
//!   [`REAUTH_TTL`], the daemon refuses to resolve and instead broadcasts
//!   `WebReauthRequired` back to the bridge so the browser can pop a sudo
//!   modal. Non-sudo tools pass through unchanged.
//! - TUI-origin approvals (device_id = None) are implicitly trusted and
//!   bypass the gate entirely. Local physical access has always been the
//!   trust boundary for the TUI; tightening that is out of scope for #218.
//!
//! The registry is in-memory by design. Persisting reauth state across
//! daemon restarts would let a stolen device that survives a restart keep
//! its sudo slot — the intent is the opposite. A restart is equivalent to
//! "everyone must reauth", which is acceptable because sudo-class actions
//! are bursty (a few per session, not per minute).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use wisphive_protocol::DeviceId;

/// Tool names that require a fresh reauth before a web device can approve
/// them. Kept as a const slice so the list is visible at a glance and can't
/// drift between the daemon and any future policy-engine callers.
///
/// The set matches the tools Claude Code actually runs on the user's
/// machine with persistent effects:
/// - `Bash` — arbitrary shell execution
/// - `Write` / `Edit` / `MultiEdit` — file mutations
/// - `NotebookEdit` — notebook cell mutations (file write by another name)
/// - `ConfigChange` — updates `~/.wisphive/config.json` (mode flips,
///   auto-approve list edits); exfiltrating this list is how an attacker
///   would broaden blast radius silently.
/// - `SpawnAgent` — launches a long-lived child with workspace-write access.
///
/// Read-only tools (Read, Grep, Glob, WebFetch, WebSearch) are intentionally
/// not gated: the UX cost of a sudo prompt isn't worth the marginal
/// protection on reads.
pub const SUDO_TOOLS: &[&str] = &[
    "Bash",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "ConfigChange",
    "SpawnAgent",
];

/// How long a reauth "counts" for. After this, approvals of sudo-class
/// tools from the device require a fresh password entry.
///
/// 5 minutes balances ergonomics against blast-radius: long enough that an
/// active operator approving several Bash commands in a row isn't prompted
/// mid-flow, short enough that a tab left open in a coffee shop isn't a
/// standing privilege grant.
pub const REAUTH_TTL: Duration = Duration::from_secs(5 * 60);

/// Returns `true` when `tool_name` is sudo-class. Case-sensitive match —
/// Claude Code emits these names verbatim.
pub fn is_sudo_tool(tool_name: &str) -> bool {
    SUDO_TOOLS.contains(&tool_name)
}

/// Per-device "last fresh reauth" timestamps.
///
/// Cheap to clone (Arc internally). The daemon holds one instance and hands
/// clones to every connection handler; the web crate's `/api/auth/reauth`
/// endpoint signals the daemon over the socket with a `MarkDeviceFresh`
/// command, which the TUI dispatch loop feeds into [`Self::touch`].
#[derive(Clone, Default)]
pub struct ReauthRegistry {
    inner: Arc<Mutex<HashMap<DeviceId, Instant>>>,
    ttl: Duration,
}

impl ReauthRegistry {
    /// Build a registry with the default [`REAUTH_TTL`].
    pub fn new() -> Self {
        Self::with_ttl(REAUTH_TTL)
    }

    /// Build with a custom TTL. Exposed for tests that need to simulate
    /// expiry without sleeping.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    /// Mark `device_id` as freshly reauthenticated right now. Also evicts
    /// any other entries whose TTL has expired so the map stays bounded
    /// without a dedicated sweep task.
    pub async fn touch(&self, device_id: &DeviceId) {
        let mut map = self.inner.lock().await;
        let now = Instant::now();
        map.retain(|_, ts| now.duration_since(*ts) < self.ttl);
        map.insert(device_id.clone(), now);
    }

    /// Returns `true` if `device_id` has a reauth within the TTL window.
    ///
    /// Also prunes the entry if it's stale, so a cold-path freshness check
    /// cleans up after itself without needing a background task.
    pub async fn is_fresh(&self, device_id: &DeviceId) -> bool {
        let mut map = self.inner.lock().await;
        match map.get(device_id) {
            Some(ts) if ts.elapsed() < self.ttl => true,
            Some(_) => {
                map.remove(device_id);
                false
            }
            None => false,
        }
    }

    /// Remove a device's freshness entry. Called when a device is revoked
    /// so the next approve doesn't accidentally ride a stale entry to
    /// success.
    pub async fn forget(&self, device_id: &DeviceId) {
        let mut map = self.inner.lock().await;
        map.remove(device_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.to_string())
    }

    #[test]
    fn sudo_tool_list_matches_plan() {
        assert!(is_sudo_tool("Bash"));
        assert!(is_sudo_tool("Write"));
        assert!(is_sudo_tool("Edit"));
        assert!(is_sudo_tool("MultiEdit"));
        assert!(is_sudo_tool("NotebookEdit"));
        assert!(is_sudo_tool("ConfigChange"));
        assert!(is_sudo_tool("SpawnAgent"));

        // Non-sudo tools
        assert!(!is_sudo_tool("Read"));
        assert!(!is_sudo_tool("Grep"));
        assert!(!is_sudo_tool("Glob"));
        assert!(!is_sudo_tool("WebFetch"));

        // Case-sensitive: protocol uses exact Claude Code names.
        assert!(!is_sudo_tool("bash"));
        assert!(!is_sudo_tool("BASH"));
    }

    #[tokio::test]
    async fn fresh_after_touch() {
        let reg = ReauthRegistry::new();
        let d = dev("a");
        assert!(!reg.is_fresh(&d).await);
        reg.touch(&d).await;
        assert!(reg.is_fresh(&d).await);
    }

    #[tokio::test]
    async fn stale_after_ttl_expiry() {
        let reg = ReauthRegistry::with_ttl(Duration::from_millis(20));
        let d = dev("a");
        reg.touch(&d).await;
        sleep(Duration::from_millis(40));
        assert!(
            !reg.is_fresh(&d).await,
            "entry older than ttl should read as stale"
        );
    }

    #[tokio::test]
    async fn stale_entries_are_pruned_on_check() {
        let reg = ReauthRegistry::with_ttl(Duration::from_millis(20));
        let d = dev("prune-me");
        reg.touch(&d).await;
        sleep(Duration::from_millis(40));
        // First call evicts.
        assert!(!reg.is_fresh(&d).await);
        // Internal map should now be empty for this key.
        {
            let map = reg.inner.lock().await;
            assert!(!map.contains_key(&d));
        }
    }

    #[tokio::test]
    async fn forget_removes_entry() {
        let reg = ReauthRegistry::new();
        let d = dev("revoke-me");
        reg.touch(&d).await;
        assert!(reg.is_fresh(&d).await);
        reg.forget(&d).await;
        assert!(!reg.is_fresh(&d).await);
    }

    #[tokio::test]
    async fn touch_evicts_other_stale_entries() {
        let reg = ReauthRegistry::with_ttl(Duration::from_millis(20));
        let stale = dev("stale");
        reg.touch(&stale).await;
        sleep(Duration::from_millis(40));
        let fresh = dev("fresh");
        reg.touch(&fresh).await;
        let map = reg.inner.lock().await;
        assert!(
            !map.contains_key(&stale),
            "stale entry should have been pruned by touch"
        );
        assert!(map.contains_key(&fresh));
    }

    #[tokio::test]
    async fn registry_clones_share_state() {
        let reg1 = ReauthRegistry::new();
        let reg2 = reg1.clone();
        let d = dev("shared");
        reg1.touch(&d).await;
        assert!(reg2.is_fresh(&d).await, "clone must see the same state");
    }
}
