//! Authentication primitives for the local web UI.
//!
//! Three pieces:
//! - **Password hashing** using Argon2id (PHC string output).
//! - **Device tokens** — opaque random bearer tokens; only the SHA-256 hash
//!   is persisted. The raw token is shown to the client exactly once.
//! - **Login throttle** — per-IP exponential backoff with bounded memory and
//!   atomic check-and-reserve, so brute force is slow *and* concurrent
//!   attempts from one IP can't dodge the lockout.
//!
//! Constant-time comparison uses `subtle::ConstantTimeEq` rather than a
//! hand-rolled XOR-OR loop: the compiler is free to vectorize the latter
//! into something with a data-dependent early exit, defeating the whole
//! point. `subtle` is already in our transitive deps via `argon2`, so the
//! direct dependency adds zero compile cost.
//
// TODO(webauthn): passkey register/verify

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::warn;
use wisphive_daemon::state::StateDb;

// ---------------------------------------------------------------------------
// Password hashing
// ---------------------------------------------------------------------------

/// Argon2id parameters: m=19456 KiB, t=2, p=1.
///
/// These are the OWASP recommended minimums for interactive logins. The
/// `Params::new` call is fallible only if values are out of range, so we
/// build it once and propagate any error.
fn argon2_instance() -> anyhow::Result<Argon2<'static>> {
    let params = Params::new(19_456, 2, 1, None)
        .map_err(|e| anyhow::anyhow!("invalid argon2 params: {e}"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash a password with Argon2id, returning a PHC-formatted string.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    // Generate a 16-byte salt from the OS RNG and base64-encode it as PHC
    // expects. Doing this by hand avoids tangling with `rand_core` feature
    // re-exports between the `argon2` and `rand` crates.
    let mut salt_bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut salt_bytes)
        .map_err(|e| anyhow::anyhow!("OS RNG failure generating salt: {e}"))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| anyhow::anyhow!("salt encoding failure: {e}"))?;
    let argon2 = argon2_instance()?;
    let phc = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash failure: {e}"))?
        .to_string();
    Ok(phc)
}

/// Verify a password against a stored PHC string. Returns `false` on any
/// parse or mismatch error — never panics.
pub fn verify_password(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    // `verify_password` does the constant-time compare internally.
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// True when no web admin password has been stored — i.e. this is a fresh
/// install and the CLI entrypoints should open a browser onto the SPA so
/// the user is dropped straight into the setup flow (itr#267).
///
/// Inherently racy: a concurrent `wisphive web set-password` can flip the
/// answer between the check and the caller acting on it. Callers must treat
/// this as advisory — the SPA itself probes `/api/auth/status` on load and
/// will route to Login instead of Onboarding when a password has already
/// been set, so the race is self-healing.
pub async fn is_first_run(db: &StateDb) -> bool {
    match db.get_web_password_hash().await {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(e) => {
            // Fail-closed on DB error: we'd rather NOT auto-open a browser
            // than pop one on a corrupted / permissions-wrong DB and
            // confuse the operator. The daemon itself will surface the
            // real error when it tries to use the DB for anything else.
            warn!(
                error = %e,
                "is_first_run: failed to read web password hash; assuming NOT first-run"
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Device tokens
// ---------------------------------------------------------------------------

/// A freshly minted device token. Hand `raw` to the client exactly once,
/// store `hash_hex` server-side.
#[derive(Debug, Clone)]
pub struct GeneratedToken {
    /// Base64url (no padding) of >=32 random bytes.
    pub raw: String,
    /// Lowercase hex of `sha256(raw.as_bytes())`. Always 64 chars.
    pub hash_hex: String,
}

/// Generate a new device token. Panics only if the OS RNG itself fails,
/// which on a healthy host should never happen.
pub fn generate_device_token() -> GeneratedToken {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG failure while generating device token");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let hash_hex = sha256_hex(raw.as_bytes());
    GeneratedToken { raw, hash_hex }
}

/// Constant-time check that `sha256(presented_raw) == stored_hash_hex`.
///
/// Returns `false` on length mismatch and `false` if the stored hash isn't
/// a 64-char string. Comparison is performed with `subtle::ConstantTimeEq`,
/// which is the only portable way to get a real CT compare — a hand-rolled
/// XOR-OR loop is correct in source but the compiler may vectorize it into
/// something with a data-dependent early exit.
pub fn verify_device_token(presented_raw: &str, stored_hash_hex: &str) -> bool {
    let computed = sha256_hex(presented_raw.as_bytes());
    constant_time_eq_str(&computed, stored_hash_hex)
}

/// SHA-256 → lowercase hex.
pub(crate) fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        // `format!` per byte is fine; this is not on a hot path and it
        // avoids pulling in the `hex` crate.
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Constant-time byte compare via `subtle::ConstantTimeEq`. Returns `false`
/// on length mismatch (but the `ct_eq` call itself short-circuits on
/// length, which is fine — the *length* of a hex-encoded SHA-256 is always
/// 64 in practice, so length-leaking here doesn't reveal anything useful).
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

// ---------------------------------------------------------------------------
// Login throttle
// ---------------------------------------------------------------------------

/// Hard cap on the number of distinct (normalized) IPs we'll track. Past
/// this we evict by oldest `last_seen`. Keeps a spray attack from growing
/// the map without bound.
const MAX_THROTTLE_ENTRIES: usize = 10_000;
/// Above this watermark, every write triggers an opportunistic sweep of
/// expired entries before we consider hard-cap eviction. Lower than the
/// hard cap so the steady state has slack.
const SWEEP_HIGH_WATER: usize = 1_000;
/// When the hard cap fires, evict this fraction of the cap so the next
/// ~MAX/EVICTION_DENOMINATOR inserts don't have to repeat the work.
/// Picking 10 means we evict 10% per overflow round.
const EVICTION_DENOMINATOR: usize = 10;
/// How long after `locked_until` we keep an entry around, so a series of
/// failed attempts spaced just past the backoff window still accumulates
/// the same `failures` count rather than starting fresh each time.
const SWEEP_GRACE: Duration = Duration::from_secs(60);
/// Cap on simultaneous in-flight verify operations per (normalized) IP.
/// 1 means: while one Argon2 verify is running for an IP, all others from
/// that IP are throttled. Closes the parallel-attempts race.
// TODO(itr#243): make configurable per-deployment so NAT'd offices /
// mobile carriers can raise this without recompiling.
const MAX_IN_FLIGHT_PER_IP: u32 = 1;
/// How long an in-flight slot is allowed to be considered "live" before
/// the eviction sweep treats it as a leaked guard and drops the entry
/// anyway. Without this, a hung verify (or a `Drop` that couldn't grab
/// `try_write`) would pin a map entry permanently and self-DoS the IP.
const STALE_IN_FLIGHT_AGE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy)]
struct AttemptState {
    failures: u32,
    locked_until: Instant,
    /// Currently-being-verified attempts from this normalized IP. Bumped
    /// by `try_begin_attempt`, decremented by `record_failure` /
    /// `record_success` / `AttemptGuard::Drop`.
    in_flight: u32,
    /// Most recent activity from this IP, used as the LRU key when we
    /// have to evict for the hard cap.
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct ThrottleInner {
    map: HashMap<IpAddr, AttemptState>,
}

/// Per-IP login throttle with exponential backoff and bounded memory.
///
/// Concurrency model: callers acquire an [`AttemptGuard`] via
/// [`Self::try_begin_attempt`] before doing any work that could be a login
/// attempt (e.g. Argon2 verify of a presented password). The guard reserves
/// an in-flight slot atomically; consume it with `record_failure` or
/// `record_success` when the verify completes. If the guard is dropped
/// without either, the slot is best-effort released.
///
/// IPv6 addresses are aggregated to /64 buckets — a /64 is the smallest
/// prefix typically routed to a single end-user, so an attacker controlling
/// a larger /48 still produces only 65,536 buckets in the worst case, well
/// under [`MAX_THROTTLE_ENTRIES`].
#[derive(Debug, Clone, Default)]
pub struct LoginThrottle {
    inner: Arc<RwLock<ThrottleInner>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ThrottleDecision {
    pub allowed: bool,
    pub retry_after: Option<Duration>,
}

/// Reservation handle returned by [`LoginThrottle::try_begin_attempt`]. The
/// reservation holds an in-flight slot for the IP that races with other
/// attempts from the same IP. Consume with [`AttemptGuard::record_failure`]
/// or [`AttemptGuard::record_success`].
///
/// **Drop is fail-closed:** dropping without explicit consumption (e.g. on
/// panic, or because the surrounding async task was cancelled) is treated
/// as an *implicit failure* — the in-flight slot is released AND the
/// failure counter is bumped. Without this, an attacker who can cancel
/// the verify (close TCP after the request line) gets unlimited tries
/// with zero backoff. If the runtime can't acquire the lock in `Drop`
/// (`try_write` failure), we warn and leak the slot to next-sweep
/// recovery rather than blocking — see [`STALE_IN_FLIGHT_AGE`].
#[derive(Debug)]
#[must_use = "An AttemptGuard reserves an in-flight slot for an IP — call record_failure or record_success to release it explicitly. Dropping without consumption is treated as a failure."]
pub struct AttemptGuard {
    inner: Arc<RwLock<ThrottleInner>>,
    /// Already-normalized (IPv6 → /64) IP key.
    ip_key: IpAddr,
    consumed: bool,
}

impl LoginThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only retry hint for UI banners: returns `Some(duration)` when
    /// the IP is currently locked out and the user should be told how long
    /// to wait. Returns `None` when the throttle has nothing to say.
    ///
    /// This intentionally does NOT return an `allowed: bool` — exposing
    /// one would invite callers to gate on `peek`'s reply, reintroducing
    /// the check-then-act race that [`Self::try_begin_attempt`] exists
    /// to fix. Always go through `try_begin_attempt` for actual
    /// admission decisions.
    pub async fn peek(&self, ip: IpAddr) -> Option<Duration> {
        let key = normalize_ip(ip);
        let state = self.inner.read().await;
        let s = state.map.get(&key)?;
        let now = Instant::now();
        if now < s.locked_until {
            Some(s.locked_until - now)
        } else {
            None
        }
    }

    /// Atomically check the throttle and, if allowed, reserve an in-flight
    /// slot for `ip`.
    ///
    /// Returns `Err(decision)` if:
    /// - the IP is currently locked out (with `retry_after = locked_until - now`),
    /// - there's already a verify in flight for the same IP (250ms retry hint),
    /// - the throttle map is at its hard cap and no entries can be evicted
    ///   to make room (1s retry hint — system is in degraded mode under
    ///   what looks like a coordinated attack).
    ///
    /// On `Ok(guard)` the caller MUST call `record_failure` or
    /// `record_success` on the guard once verify finishes; dropping the
    /// guard without that is treated as an implicit failure.
    pub async fn try_begin_attempt(&self, ip: IpAddr) -> Result<AttemptGuard, ThrottleDecision> {
        let key = normalize_ip(ip);
        let mut state = self.inner.write().await;
        let now = Instant::now();

        // Look at the existing entry without inserting one — we don't want
        // a peek that returns Err to leave a permanent map entry behind.
        if let Some(s) = state.map.get(&key) {
            if now < s.locked_until {
                return Err(ThrottleDecision {
                    allowed: false,
                    retry_after: Some(s.locked_until - now),
                });
            }
            if s.in_flight >= MAX_IN_FLIGHT_PER_IP {
                // Another verify is in flight; reject. Use a small
                // retry_after so a polite client backs off briefly.
                return Err(ThrottleDecision {
                    allowed: false,
                    retry_after: Some(Duration::from_millis(250)),
                });
            }
        }

        // We're going to grant the attempt — make room for a new entry
        // if needed. Existing entries don't grow the map so the sweep
        // is only required when we'd be admitting a new IP, but the
        // check is cheap and centralizes the policy here.
        //
        // If `ensure_room_for_new_entry` can't free a slot (every entry
        // is in_flight > 0 and recent), we MUST refuse to insert — admitting
        // anyway would let an attacker holding `in_flight` slots on N
        // distinct /64s grow the map past `MAX_THROTTLE_ENTRIES`,
        // regressing the bound itr#229 was meant to enforce.
        let is_new_entry = !state.map.contains_key(&key);
        if is_new_entry && !ensure_room_for_new_entry(&mut state, now) {
            warn!(
                map_len = state.map.len(),
                "throttle map at hard cap with no evictable entries; rejecting new attempt"
            );
            return Err(ThrottleDecision {
                allowed: false,
                retry_after: Some(Duration::from_secs(1)),
            });
        }

        let entry = state.map.entry(key).or_insert(AttemptState {
            failures: 0,
            locked_until: now,
            in_flight: 0,
            last_seen: now,
        });
        entry.in_flight = entry.in_flight.saturating_add(1);
        entry.last_seen = now;

        Ok(AttemptGuard {
            inner: self.inner.clone(),
            ip_key: key,
            consumed: false,
        })
    }
}

impl AttemptGuard {
    /// Record that the verify failed. Increments the per-IP failure count,
    /// pushes `locked_until` forward per the backoff schedule, and releases
    /// the in-flight slot.
    pub async fn record_failure(mut self) {
        self.consumed = true;
        let mut state = self.inner.write().await;
        apply_failure(&mut state, self.ip_key);
    }

    /// Record that the verify succeeded. Drops the per-IP entry entirely
    /// — a successful login wipes the lockout history.
    pub async fn record_success(mut self) {
        self.consumed = true;
        let mut state = self.inner.write().await;
        state.map.remove(&self.ip_key);
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if self.consumed {
            return;
        }
        // Fail-closed: a guard dropped without explicit consumption is
        // treated as an implicit failure. Without this, an attacker that
        // can cancel mid-verify (close TCP after the request line) gets
        // unlimited tries with zero backoff — exactly the bypass the
        // try_begin_attempt API was designed to prevent.
        //
        // We're in sync code, so we can't `.await` the write lock; use
        // `try_write`. On contention we log and leak the in_flight slot
        // — `STALE_IN_FLIGHT_AGE` in `ensure_room_for_new_entry` will
        // reap it within a few minutes, so this can't permanently
        // throttle the IP.
        match self.inner.try_write() {
            Ok(mut state) => apply_failure(&mut state, self.ip_key),
            Err(_) => {
                warn!(
                    ip = %self.ip_key,
                    "AttemptGuard dropped without record under contention; \
                     fail-closed bookkeeping deferred until STALE_IN_FLIGHT_AGE sweep"
                );
            }
        }
    }
}

/// Sync helper shared by `record_failure` (async) and `Drop` (sync). Bumps
/// the failure counter, pushes `locked_until`, and releases one in-flight
/// slot. No-op if the entry has been evicted under us (which shouldn't
/// happen because eviction skips `in_flight > 0`, but be defensive).
fn apply_failure(state: &mut ThrottleInner, ip_key: IpAddr) {
    let now = Instant::now();
    if let Some(entry) = state.map.get_mut(&ip_key) {
        entry.failures = entry.failures.saturating_add(1);
        entry.locked_until = now + backoff_for(entry.failures);
        entry.in_flight = entry.in_flight.saturating_sub(1);
        entry.last_seen = now;
    }
}

/// Backoff schedule: `min(30s, 250ms * 2^(N-1))`.
fn backoff_for(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let cap = Duration::from_secs(30);
    // 2^(N-1) — saturating so we don't UB on huge N.
    let mult = 2u32.saturating_pow(failures - 1);
    let base = Duration::from_millis(250);
    base.checked_mul(mult).map(|d| d.min(cap)).unwrap_or(cap)
}

/// Aggregate IPv6 addresses to /64. IPv4 addresses (including IPv4-mapped
/// IPv6 like `::ffff:a.b.c.d` that axum hands us on dual-stack listeners)
/// are passed through as their /32.
///
/// Without the v4-mapped unmap step, every IPv4 client behind a dual-stack
/// listener would collapse to a single `::` /64 bucket and share lockout
/// state — a self-DoS waiting to happen the moment any one IPv4 client
/// fails to log in.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    let ip = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 @ IpAddr::V4(_) => v4,
    };
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            let mut masked = [0u8; 16];
            masked[..8].copy_from_slice(&octets[..8]);
            IpAddr::V6(Ipv6Addr::from(masked))
        }
    }
}

/// Make room for one more entry in the throttle map without exceeding
/// [`MAX_THROTTLE_ENTRIES`]. Returns `true` if there is room (or if no
/// eviction was needed); `false` if the cap is full and nothing could
/// be evicted, in which case the caller MUST refuse to insert.
///
/// Three passes, cheapest first:
/// 1. **Grace sweep** (cheap, O(N)): drop entries whose lockout has
///    expired more than [`SWEEP_GRACE`] ago and that aren't holding an
///    in-flight slot. Steady-state cleanup.
/// 2. **Stale-in-flight reaper** (folded into the same retain): drop
///    entries whose `in_flight > 0` *and* whose `last_seen` is older
///    than [`STALE_IN_FLIGHT_AGE`]. Treats those as leaked guards (a
///    `Drop` that hit `try_write` contention, or a runaway verify) —
///    without this, a self-DoS path keeps the IP locked forever.
/// 3. **Hard-cap eviction** (O(N), batched): if we're still at or above
///    the cap after sweeping, batch-evict the oldest evictable entries
///    by `last_seen`. Batching means the next ~MAX/EVICTION_DENOMINATOR
///    inserts pay no eviction cost.
///
/// Returning `bool` is the MUST-FIX motivator: if every entry has
/// `in_flight > 0` AND each is too young for the stale sweep, the
/// previous version early-returned and `try_begin_attempt` would insert
/// anyway, growing the map past the cap. Now we report failure and the
/// caller refuses the new attempt — which preserves itr#229's bound
/// even under coordinated attack load.
fn ensure_room_for_new_entry(state: &mut ThrottleInner, now: Instant) -> bool {
    if state.map.len() >= SWEEP_HIGH_WATER {
        state.map.retain(|_, s| {
            if s.in_flight > 0 {
                // Stale-in-flight reaper: a slot held longer than
                // STALE_IN_FLIGHT_AGE is treated as leaked. We trade the
                // bookkeeping update for whatever guard is actually
                // still mid-verify against avoiding permanent self-DoS
                // for the IP.
                let age = now.saturating_duration_since(s.last_seen);
                return age < STALE_IN_FLIGHT_AGE;
            }
            match s.locked_until.checked_add(SWEEP_GRACE) {
                Some(deadline) => now < deadline,
                None => true,
            }
        });
    }

    if state.map.len() < MAX_THROTTLE_ENTRIES {
        return true;
    }

    let target_after =
        MAX_THROTTLE_ENTRIES.saturating_sub(MAX_THROTTLE_ENTRIES / EVICTION_DENOMINATOR);
    let want_to_evict = state.map.len().saturating_sub(target_after);

    let mut evictable: Vec<(IpAddr, Instant)> = state
        .map
        .iter()
        .filter(|(_, s)| s.in_flight == 0)
        .map(|(k, s)| (*k, s.last_seen))
        .collect();
    if evictable.is_empty() {
        // Every entry is in_flight > 0 and not yet stale. Caller MUST
        // refuse the new attempt — admitting anyway would grow the map
        // past the cap and regress itr#229 under coordinated attack load.
        return false;
    }
    let n = want_to_evict.min(evictable.len());
    if n == 0 {
        return state.map.len() < MAX_THROTTLE_ENTRIES;
    }
    debug_assert!(n >= 1 && n <= evictable.len());
    // Partition (not full sort) so the n oldest are at the front. O(N)
    // average, O(N²) worst case; the chaotic order from HashMap iteration
    // makes the worst case unreachable in practice.
    let nth_idx = n - 1;
    evictable.select_nth_unstable_by_key(nth_idx, |&(_, ts)| ts);
    for (k, _) in evictable.into_iter().take(n) {
        state.map.remove(&k);
    }
    state.map.len() < MAX_THROTTLE_ENTRIES
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn argon2_roundtrip() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &phc));
        assert!(!verify_password("wrong password", &phc));
        assert!(!verify_password("", &phc));
    }

    #[test]
    fn token_uniqueness() {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let t = generate_device_token();
            assert_eq!(t.hash_hex.len(), 64, "hash_hex must be 64 chars");
            assert!(
                t.hash_hex
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "hash_hex must be lowercase hex: {}",
                t.hash_hex
            );
            assert!(seen.insert(t.raw), "duplicate raw token");
        }
    }

    #[test]
    fn token_verify_roundtrip() {
        let t = generate_device_token();
        assert!(verify_device_token(&t.raw, &t.hash_hex));
        assert!(!verify_device_token("not-the-token", &t.hash_hex));
        let truncated = &t.raw[..t.raw.len() - 4];
        assert!(!verify_device_token(truncated, &t.hash_hex));
    }

    #[test]
    fn constant_time_compare_unequal_lengths() {
        // hash_hex is 64 chars; pass a too-short stored hash and a too-long one.
        let t = generate_device_token();
        assert!(!verify_device_token(&t.raw, "deadbeef"));
        let too_long = format!("{}ff", t.hash_hex);
        assert!(!verify_device_token(&t.raw, &too_long));
        // And the helper itself shouldn't panic on empty inputs.
        assert!(constant_time_eq_str("", ""));
        assert!(!constant_time_eq_str("", "a"));
        assert!(!constant_time_eq_str("a", ""));
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_backoff_schedule() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        // N=1 → ~250ms
        throttle
            .try_begin_attempt(ip)
            .await
            .expect("first attempt should be allowed")
            .record_failure()
            .await;
        let ra = throttle.peek(ip).await.expect("locked out after N=1");
        assert!(
            ra >= Duration::from_millis(200) && ra <= Duration::from_millis(260),
            "N=1 retry_after expected ~250ms, got {ra:?}"
        );
        tokio::time::advance(Duration::from_millis(260)).await;
        assert!(throttle.peek(ip).await.is_none());

        // N=2, N=3
        throttle
            .try_begin_attempt(ip)
            .await
            .expect("post-cooldown attempt allowed")
            .record_failure()
            .await;
        // Second attempt while N=2 lockout is in effect: must be rejected
        // by the lockout, not by the in-flight cap.
        let err = throttle.try_begin_attempt(ip).await.unwrap_err();
        assert!(!err.allowed);
        // Wait out the N=2 lockout, then push to N=3.
        tokio::time::advance(Duration::from_millis(600)).await;
        throttle
            .try_begin_attempt(ip)
            .await
            .unwrap()
            .record_failure()
            .await;
        let ra = throttle.peek(ip).await.expect("locked out after N=3");
        assert!(
            ra >= Duration::from_millis(900) && ra <= Duration::from_millis(1100),
            "N=3 retry_after expected ~1s, got {ra:?}"
        );
    }

    /// itr#231 acceptance, hardened: a slow verify holds the in-flight
    /// slot deterministically (via a Notify) while 9 racers each try to
    /// begin. Exactly one should have won (the original holder), and the
    /// 9 racers must all have been rejected.
    ///
    /// The previous version of this test could false-pass on a 1-vCPU
    /// runner where the scheduler trivially serialized the spawns; this
    /// version forces overlap by keeping the winner's guard alive across
    /// the awaits.
    #[tokio::test]
    async fn parallel_attempts_one_in_flight() {
        use std::sync::Arc;
        use tokio::sync::Notify;

        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        // Winner takes the slot first and holds it until we say so.
        let winner = throttle
            .try_begin_attempt(ip)
            .await
            .expect("winner should grab the slot");

        // Spawn 9 racers; gate them behind a Notify so they all enter
        // try_begin_attempt while the winner is demonstrably still
        // holding the slot.
        let gate = Arc::new(Notify::new());
        let handles: Vec<_> = (0..9)
            .map(|_| {
                let t = throttle.clone();
                let g = gate.clone();
                tokio::spawn(async move {
                    g.notified().await;
                    t.try_begin_attempt(ip).await
                })
            })
            .collect();
        // notify_waiters wakes everyone currently waiting on this Notify.
        // To make sure they're all parked, yield first — Notify::notify_one
        // would queue notifications but notify_waiters only delivers to
        // currently-parked waiters, exactly the semantics we want.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        gate.notify_waiters();

        let mut throttled = 0usize;
        let mut accidental_winners = 0usize;
        for h in handles {
            match h.await.unwrap() {
                Ok(_) => accidental_winners += 1,
                Err(d) => {
                    assert!(!d.allowed);
                    throttled += 1;
                }
            }
        }
        assert_eq!(
            accidental_winners, 0,
            "racers should all be throttled while the original guard is held"
        );
        assert_eq!(throttled, 9, "all 9 racers should report throttled");

        winner.record_failure().await;
    }

    /// itr#231 follow-up: once the first attempt finishes (fail or
    /// success), the next caller should be able to begin (subject to the
    /// backoff for record_failure, or freely after record_success).
    #[tokio::test(start_paused = true)]
    async fn slot_releases_after_record() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        // Acquire, succeed → next call should be wide open.
        let g = throttle.try_begin_attempt(ip).await.unwrap();
        g.record_success().await;
        let g2 = throttle.try_begin_attempt(ip).await.unwrap();
        g2.record_failure().await;
        // After failure the IP is in cooldown — next try_begin_attempt
        // should be rejected by the lockout, not by the in-flight cap.
        let err = throttle.try_begin_attempt(ip).await.unwrap_err();
        assert!(!err.allowed);
    }

    /// itr#229 acceptance: spraying many distinct IPs must not grow the
    /// throttle map without bound. We use 50_000 IPs (much faster than
    /// the issue's 1M but still well above MAX_THROTTLE_ENTRIES) and
    /// assert the map stays at or below the cap.
    #[tokio::test]
    async fn map_stays_bounded_under_spray() {
        let throttle = LoginThrottle::new();
        const N: u32 = 50_000;
        for i in 0..N {
            let ip: IpAddr = Ipv4Addr::from(i.to_be_bytes()).into();
            // Use try_begin_attempt + record_failure so each IP both
            // gets an entry AND has an in_flight=0 to allow eviction.
            if let Ok(g) = throttle.try_begin_attempt(ip).await {
                g.record_failure().await;
            }
        }
        let len = throttle.inner.read().await.map.len();
        assert!(
            len <= MAX_THROTTLE_ENTRIES,
            "throttle map grew to {len} entries; cap is {MAX_THROTTLE_ENTRIES}"
        );
    }

    /// itr#229 follow-up: an attacker spraying from a /64 should produce
    /// exactly one map entry, not 2^64.
    #[tokio::test]
    async fn ipv6_aggregates_to_slash_64() {
        let throttle = LoginThrottle::new();
        // Two addresses in the same /64 (`2001:db8:1:2::*`) and one in a
        // different /64 (`2001:db8:1:3::1`). After registering them all
        // we should see exactly two entries: one per /64.
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::beef".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        for ip in [a, b, c] {
            if let Ok(g) = throttle.try_begin_attempt(ip).await {
                g.record_failure().await;
            }
        }
        let len = throttle.inner.read().await.map.len();
        assert_eq!(len, 2, "expected two /64 buckets, got {len}");

        // And: the two /64-mates must share lockout state.
        let da = throttle.peek(a).await;
        let db = throttle.peek(b).await;
        assert_eq!(da.is_some(), db.is_some());
    }

    /// IPv4 should NOT be aggregated — two different /32s must produce
    /// two map entries.
    #[tokio::test]
    async fn ipv4_not_aggregated() {
        let throttle = LoginThrottle::new();
        let a: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();
        for ip in [a, b] {
            if let Ok(g) = throttle.try_begin_attempt(ip).await {
                g.record_failure().await;
            }
        }
        let len = throttle.inner.read().await.map.len();
        assert_eq!(len, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_success_clears() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        // Two failures then a success.
        for _ in 0..2 {
            let g = throttle.try_begin_attempt(ip).await;
            // Second iteration may be locked out — wait it out.
            match g {
                Ok(g) => g.record_failure().await,
                Err(d) => {
                    if let Some(ra) = d.retry_after {
                        tokio::time::advance(ra + Duration::from_millis(10)).await;
                        let g = throttle.try_begin_attempt(ip).await.unwrap();
                        g.record_failure().await;
                    }
                }
            }
        }
        // Wait out whatever lockout is in place from the last failure.
        if let Some(ra) = throttle.peek(ip).await {
            tokio::time::advance(ra + Duration::from_millis(10)).await;
        }
        let g = throttle.try_begin_attempt(ip).await.unwrap();
        g.record_success().await;
        assert!(throttle.peek(ip).await.is_none());
        assert!(!throttle.inner.read().await.map.contains_key(&ip));
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_per_ip_independent() {
        let throttle = LoginThrottle::new();
        let bad: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let good: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();

        // Push `bad` into deep lockout.
        for _ in 0..3 {
            if let Some(ra) = throttle.peek(bad).await {
                tokio::time::advance(ra + Duration::from_millis(10)).await;
            }
            throttle
                .try_begin_attempt(bad)
                .await
                .unwrap()
                .record_failure()
                .await;
        }
        assert!(throttle.peek(bad).await.is_some());
        assert!(throttle.peek(good).await.is_none());
    }

    #[test]
    fn backoff_schedule_values() {
        // Spot-check a few rungs of the ladder.
        assert_eq!(backoff_for(1), Duration::from_millis(250));
        assert_eq!(backoff_for(2), Duration::from_millis(500));
        assert_eq!(backoff_for(3), Duration::from_secs(1));
        assert_eq!(backoff_for(4), Duration::from_secs(2));
        assert_eq!(backoff_for(5), Duration::from_secs(4));
        assert_eq!(backoff_for(6), Duration::from_secs(8));
        assert_eq!(backoff_for(7), Duration::from_secs(16));
        assert_eq!(backoff_for(8), Duration::from_secs(30));
        assert_eq!(backoff_for(20), Duration::from_secs(30));
        assert_eq!(backoff_for(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn normalize_ipv4_passthrough() {
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 1).into();
        assert_eq!(normalize_ip(ip), ip);
    }

    #[test]
    fn normalize_ipv6_zeros_lower_64() {
        let ip: IpAddr = "2001:db8:1:2:abcd:ef01:2345:6789"
            .parse::<Ipv6Addr>()
            .unwrap()
            .into();
        let n = normalize_ip(ip);
        let expected: IpAddr = "2001:db8:1:2::".parse::<Ipv6Addr>().unwrap().into();
        assert_eq!(n, expected);
    }

    /// Drop without record_* must (a) release the in-flight slot AND
    /// (b) record an implicit failure — anything else lets an attacker
    /// who can cancel mid-verify dodge the lockout entirely.
    #[tokio::test]
    async fn drop_without_record_records_failure() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        {
            let _g = throttle.try_begin_attempt(ip).await.unwrap();
            // Sanity: a second concurrent attempt is throttled while the
            // first is in flight.
            let err = throttle.try_begin_attempt(ip).await.unwrap_err();
            assert!(!err.allowed);
        } // _g dropped here without record → fail-closed bumps failure

        // The in-flight slot must be released (next try_begin doesn't
        // hit the in_flight cap)…
        let err = throttle.try_begin_attempt(ip).await.unwrap_err();
        // …but we should now be in lockout, NOT wide open. Without the
        // fail-closed Drop, this would be Ok.
        assert!(
            !err.allowed,
            "fail-closed Drop should leave the IP in lockout"
        );
        assert!(
            err.retry_after.is_some(),
            "lockout should have a retry_after"
        );

        // And the failure was recorded in the map.
        let state = throttle.inner.read().await;
        let entry = state
            .map
            .get(&normalize_ip(ip))
            .expect("entry should still exist after fail-closed Drop");
        assert_eq!(entry.failures, 1);
        assert_eq!(entry.in_flight, 0);
    }

    /// itr#229 hardening (review M1/#8): when the map is full of
    /// in-flight entries (every entry pinned by a held guard), a new
    /// attempt from a previously-unseen IP must be REJECTED rather than
    /// admitted unbounded. The previous version of
    /// `ensure_room_for_new_entry` would early-return and the caller
    /// would insert anyway, defeating the cap.
    ///
    /// We can't easily fill the cap to 10k under in-flight in a test
    /// (would need 10k tasks holding guards), so we lower the bar
    /// surgically: pre-populate the map up to MAX with in-flight=1
    /// entries, then assert the next try_begin_attempt with a fresh IP
    /// returns Err and the map size stays at MAX.
    #[tokio::test]
    async fn rejects_new_attempt_when_cap_full_of_in_flight() {
        let throttle = LoginThrottle::new();

        // Fill the map by directly poking the inner state (the
        // alternative is spawning 10k tasks, which is prohibitively
        // slow). All entries get in_flight=1 so eviction can't touch them.
        {
            let mut state = throttle.inner.write().await;
            let now = Instant::now();
            for i in 0..MAX_THROTTLE_ENTRIES {
                let ip: IpAddr = Ipv4Addr::from((i as u32).to_be_bytes()).into();
                state.map.insert(
                    ip,
                    AttemptState {
                        failures: 0,
                        locked_until: now,
                        in_flight: 1,
                        last_seen: now,
                    },
                );
            }
        }

        // Anyone new should be told to retry, NOT admitted.
        let stranger: IpAddr = Ipv4Addr::new(203, 0, 113, 7).into();
        let err = throttle
            .try_begin_attempt(stranger)
            .await
            .expect_err("cap-full-of-in-flight should reject new attempt");
        assert!(!err.allowed);
        assert!(err.retry_after.is_some());

        // Map size must not have grown past the cap.
        let len = throttle.inner.read().await.map.len();
        assert_eq!(
            len, MAX_THROTTLE_ENTRIES,
            "map grew past cap to {len}; itr#229 bound regressed"
        );
    }

    /// itr#267: first-run detection. Fresh DB → true; after
    /// `set_web_password` → false. The CLI uses this to decide whether
    /// to auto-open the browser on `daemon start --web`.
    #[tokio::test]
    async fn is_first_run_flips_after_password_set() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wisphive.db");
        let db = StateDb::open(db_path.to_string_lossy().as_ref())
            .await
            .unwrap();

        // Fresh install: no password → first-run.
        assert!(is_first_run(&db).await, "fresh DB should be first-run");

        // After set_web_password: first-run flips false.
        let phc = hash_password("hunter2-test").unwrap();
        db.set_web_password(&phc).await.unwrap();
        assert!(
            !is_first_run(&db).await,
            "DB with password set should NOT be first-run"
        );

        // Reset brings it back to first-run — a reset should re-trigger
        // the onboarding flow, which matches the UX the CLI wires up.
        db.reset_web_password().await.unwrap();
        assert!(
            is_first_run(&db).await,
            "DB after reset_web_password should be first-run again"
        );
    }

    /// itr#229 hardening (review M2): IPv4-mapped IPv6 addresses
    /// (`::ffff:a.b.c.d`) must unmap to v4 *before* /64 normalization.
    /// Without this, every IPv4 client behind a dual-stack listener
    /// collapses to a single `::` /64 bucket and shares lockout state.
    #[test]
    fn normalize_unmaps_v4_mapped_v6() {
        let mapped: IpAddr = "::ffff:192.0.2.1".parse::<Ipv6Addr>().unwrap().into();
        let unmapped: IpAddr = Ipv4Addr::new(192, 0, 2, 1).into();
        assert_eq!(normalize_ip(mapped), unmapped);
        // And two distinct v4-mapped addresses must produce distinct keys.
        let other: IpAddr = "::ffff:192.0.2.2".parse::<Ipv6Addr>().unwrap().into();
        assert_ne!(normalize_ip(mapped), normalize_ip(other));
    }
}
