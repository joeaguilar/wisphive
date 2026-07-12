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
use std::fmt::Write as _;
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

/// Minimum acceptable Argon2 cost parameters, mirroring the params
/// `argon2_instance` hashes with. These are the *rehash trigger* for
/// `verify_password_with_migration`, **not** a hard reject wall.
///
/// # Raising a floor is not free
///
/// Historically these gated `verify_password`: any stored hash below the
/// floor made the correct password verify `false` *forever*, with no
/// rehash-on-verify migration path (a one-way ratchet — itr#502). That made
/// bumping a floor a foot-gun: every account whose stored hash predated the
/// bump (an imported DB, an older build, or simply a hash minted before the
/// bump) would be **silently locked out**, indistinguishable from a wrong
/// password.
///
/// The read path no longer rejects on the cost floor: a correct password
/// verifies against the hash's *own embedded* parameters and the outcome
/// flags that a rehash is warranted (`OkRehashNeeded`, see
/// `verify_password_with_migration`), so raising a floor no longer *locks
/// out* below-floor accounts. The *write* path (`argon2_instance`) still
/// mints at these floors, so newly stored hashes are never below them.
///
/// Web password handlers consume `OkRehashNeeded` and transparently replace
/// the stored hash at the current parameters while the cleartext is still in
/// hand. The persistence update is a compare-and-swap against the verified
/// PHC string, so a concurrent login, password change, or reset cannot be
/// clobbered by a stale migration. Do not reintroduce a bare below-floor
/// `return false`.
const MIN_M_COST: u32 = 19_456;
const MIN_T_COST: u32 = 2;
const MIN_P_COST: u32 = 1;

/// Outcome of verifying a candidate password against a stored PHC hash.
///
/// Distinguishes a plain mismatch from a *correct* password whose stored hash
/// was minted below the current `MIN_*_COST` floor and so warrants a
/// transparent rehash on this successful verification (itr#502). Web auth
/// handlers branch on `OkRehashNeeded` and re-persist `hash_password(password)`
/// while the cleartext is still in hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordVerification {
    /// Password matched and the stored hash already meets the current floor.
    Ok,
    /// Password matched, but the stored hash's embedded cost parameters are
    /// below the current floor. Treat as a **successful** login, then re-hash
    /// the still-in-hand cleartext with `hash_password` and persist it so the
    /// account is migrated off the weak parameters.
    OkRehashNeeded,
    /// Password did not match, the PHC string was unparseable, or it used a
    /// disallowed algorithm. Indistinguishable to the caller by design.
    Failed,
}

/// Verify a password against a stored PHC string. Returns `false` on any
/// parse, algorithm, or mismatch error — never panics. A correct password
/// against a below-floor Argon2id hash returns `true` (the read-path ratchet
/// of itr#502 is closed); use `verify_password_with_migration` when the
/// caller can rehash to migrate that hash forward.
pub fn verify_password(password: &str, phc: &str) -> bool {
    !matches!(
        verify_password_with_migration(password, phc),
        PasswordVerification::Failed
    )
}

/// Verify a password against a stored PHC string, surfacing whether a correct
/// password's stored hash is below the current cost floor and should be
/// rehashed.
///
/// The PHC string's algorithm is still checked against `Argon2id` up front:
/// an algorithm downgrade (e.g. a row advertising `argon2i`) is a distinct
/// concern from the cost-floor ratchet, is never produced by any in-tree
/// write path, and stays a hard `Failed`. The cost *parameters*, however, are
/// **not** a reject wall (itr#502): the candidate is verified against the
/// hash's own embedded parameters — `Argon2::default().verify_password`
/// reconstructs algorithm/params from the PHC string, so this compares
/// correctly even for a below-floor hash — and only after a *successful*
/// compare do we consult the floor. A correct password against a below-floor
/// hash returns `OkRehashNeeded` (never a permanent lockout); a wrong
/// password returns `Failed` regardless of the stored params, so no floor
/// state leaks through the boolean or triggers the rehash warning.
pub fn verify_password_with_migration(password: &str, phc: &str) -> PasswordVerification {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return PasswordVerification::Failed;
    };
    if parsed.algorithm != Algorithm::Argon2id.ident() {
        return PasswordVerification::Failed;
    }
    let Ok(params) = Params::try_from(&parsed) else {
        return PasswordVerification::Failed;
    };
    // Verify against the hash's OWN embedded parameters first. `verify_password`
    // does the constant-time compare internally.
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_err()
    {
        return PasswordVerification::Failed;
    }
    // Correct password. If the stored hash predates the current floor, the
    // login succeeds but the caller should rehash to migrate it forward. The
    // warning fires only on a successful compare, so wrong-password guessing
    // can't spam it or use it as a floor oracle.
    if params.m_cost() < MIN_M_COST || params.t_cost() < MIN_T_COST || params.p_cost() < MIN_P_COST
    {
        warn!(
            m_cost = params.m_cost(),
            t_cost = params.t_cost(),
            p_cost = params.p_cost(),
            min_m_cost = MIN_M_COST,
            min_t_cost = MIN_T_COST,
            min_p_cost = MIN_P_COST,
            "web password verified against an Argon2id hash below the current cost floor; attempting transparent rehash"
        );
        return PasswordVerification::OkRehashNeeded;
    }
    PasswordVerification::Ok
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
        // Write directly into the single pre-allocated buffer instead of
        // allocating a small `String` per byte via `format!` — this keeps
        // the whole function to one allocation and avoids pulling in the
        // `hex` crate.
        write!(out, "{b:02x}").unwrap();
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
/// Default cap on simultaneous in-flight verify operations per (normalized)
/// IP. 1 means: while one Argon2 verify is running for an IP, all others
/// from that IP are throttled. Closes the parallel-attempts race.
///
/// itr#243: NAT'd offices / mobile carriers behind a shared egress IP hit
/// this hard during login waves (every legitimate user 250ms-storms every
/// other). Configurable per-deployment via [`MAX_IN_FLIGHT_PER_IP_ENV`] —
/// see [`LoginThrottle::new`] — without recompiling. The default itself is
/// unchanged; only tightened deployments need to touch the knob.
const DEFAULT_MAX_IN_FLIGHT_PER_IP: u32 = 1;
/// Environment variable read by [`LoginThrottle::new`] to override
/// [`DEFAULT_MAX_IN_FLIGHT_PER_IP`]. Unset, empty, non-numeric, or `0`
/// values fall back to the default (itr#243).
const MAX_IN_FLIGHT_PER_IP_ENV: &str = "WISPHIVE_MAX_IN_FLIGHT_PER_IP";

/// Pure parse of the `WISPHIVE_MAX_IN_FLIGHT_PER_IP` override. Factored out
/// of [`max_in_flight_from_env`] so tests can exercise the parsing rules
/// (default fallback, valid override, invalid input) without mutating
/// process-global environment state.
fn parse_max_in_flight(raw: Option<&str>) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_MAX_IN_FLIGHT_PER_IP)
}

/// Reads [`MAX_IN_FLIGHT_PER_IP_ENV`] for the per-IP in-flight cap.
fn max_in_flight_from_env() -> u32 {
    parse_max_in_flight(std::env::var(MAX_IN_FLIGHT_PER_IP_ENV).ok().as_deref())
}

/// Ceiling on the exponential backoff schedule computed by [`backoff_for`].
/// itr#246: this used to be a hardcoded 30s. The sprint's own non-goal
/// ("no throttle calibration from real UX telemetry") means we don't yet
/// have data to justify a new hardcoded default (e.g. 5 minutes), so this
/// lands the knob — mirroring the [`DEFAULT_MAX_IN_FLIGHT_PER_IP`] /
/// [`MAX_IN_FLIGHT_PER_IP_ENV`] pattern from itr#243 — with the existing
/// 30s behavior preserved as the default. A deployment that has measured
/// its own UX can raise the cap via [`BACKOFF_CAP_SECS_ENV`] without
/// recompiling.
const DEFAULT_BACKOFF_CAP: Duration = Duration::from_secs(30);
/// Environment variable read by [`LoginThrottle::new`] to override
/// [`DEFAULT_BACKOFF_CAP`]. Unset, empty, non-numeric, or `0` values fall
/// back to the default (itr#246).
const BACKOFF_CAP_SECS_ENV: &str = "WISPHIVE_BACKOFF_CAP_SECS";

/// Pure parse of the `WISPHIVE_BACKOFF_CAP_SECS` override. Factored out of
/// [`backoff_cap_from_env`] so tests can exercise the parsing rules
/// (default fallback, valid override, invalid input) without mutating
/// process-global environment state — same shape as
/// [`parse_max_in_flight`] (itr#243).
fn parse_backoff_cap_secs(raw: Option<&str>) -> Duration {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_BACKOFF_CAP)
}

/// Reads [`BACKOFF_CAP_SECS_ENV`] for the backoff cap.
fn backoff_cap_from_env() -> Duration {
    parse_backoff_cap_secs(std::env::var(BACKOFF_CAP_SECS_ENV).ok().as_deref())
}

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
#[derive(Debug, Clone)]
pub struct LoginThrottle {
    inner: Arc<RwLock<ThrottleInner>>,
    /// Per-IP in-flight cap — see [`DEFAULT_MAX_IN_FLIGHT_PER_IP`] and
    /// [`MAX_IN_FLIGHT_PER_IP_ENV`] (itr#243).
    max_in_flight: u32,
    /// Ceiling on the exponential backoff schedule — see
    /// [`DEFAULT_BACKOFF_CAP`] and [`BACKOFF_CAP_SECS_ENV`] (itr#246).
    backoff_cap: Duration,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new()
    }
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
    /// Backoff cap carried from the owning [`LoginThrottle`] so
    /// `record_failure`/`Drop` can compute the schedule without a back
    /// reference to the throttle itself (itr#246).
    backoff_cap: Duration,
}

impl LoginThrottle {
    /// Builds a throttle with the per-IP in-flight cap taken from
    /// [`MAX_IN_FLIGHT_PER_IP_ENV`] (falling back to
    /// [`DEFAULT_MAX_IN_FLIGHT_PER_IP`] = 1 when unset/invalid — itr#243).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ThrottleInner::default())),
            max_in_flight: max_in_flight_from_env(),
            backoff_cap: backoff_cap_from_env(),
        }
    }

    /// Builds a throttle with an explicit per-IP in-flight cap, bypassing
    /// the environment variable. Primarily for tests that need a
    /// deterministic cap regardless of the process environment.
    #[cfg(test)]
    fn with_max_in_flight(max_in_flight: u32) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ThrottleInner::default())),
            max_in_flight,
            backoff_cap: DEFAULT_BACKOFF_CAP,
        }
    }

    /// Builds a throttle with an explicit backoff cap, bypassing the
    /// environment variable. Primarily for tests that need a deterministic
    /// cap regardless of the process environment (itr#246).
    #[cfg(test)]
    fn with_backoff_cap(backoff_cap: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ThrottleInner::default())),
            max_in_flight: DEFAULT_MAX_IN_FLIGHT_PER_IP,
            backoff_cap,
        }
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
            if s.in_flight >= self.max_in_flight {
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
            backoff_cap: self.backoff_cap,
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
        apply_failure(&mut state, self.ip_key, self.backoff_cap);
    }

    /// Record that the verify succeeded. Releases **only this guard's own**
    /// in-flight reservation and wipes the per-IP lockout history (a
    /// successful login clears the IP's failure counter and lockout window).
    ///
    /// itr#498: previously this unconditionally did `state.map.remove(&ip_key)`,
    /// which erased the whole per-IP entry — including the `in_flight` counts
    /// of *sibling* guards still verifying from the same IP. Harmless under
    /// the old hardcoded cap of 1 (siblings could never coexist), but once the
    /// cap is configurable > 1 (itr#243, the NAT/office scenario) one sibling's
    /// success wiped the shared counter while other siblings were still
    /// outstanding, letting fresh attempts stack on top of them so the true
    /// concurrent count could exceed the configured cap. Now we decrement in
    /// place like [`Self::release_slot`] and only remove the map entry once it
    /// is genuinely empty — no siblings still in flight — mirroring how the
    /// eviction sweep garbage-collects idle entries.
    pub async fn record_success(mut self) {
        self.consumed = true;
        let now = Instant::now();
        let mut state = self.inner.write().await;
        if let Some(entry) = state.map.get_mut(&self.ip_key) {
            // Release only our own reservation; siblings keep theirs.
            entry.in_flight = entry.in_flight.saturating_sub(1);
            // A successful login clears this IP's lockout history.
            entry.failures = 0;
            entry.locked_until = now;
            entry.last_seen = now;
            // GC the entry only when it carries no live state: no in-flight
            // siblings (and failures/lockout were just cleared above).
            if entry.in_flight == 0 {
                state.map.remove(&self.ip_key);
            }
        }
    }

    /// Release the in-flight slot without counting the attempt as either
    /// a failure or a success. Use for cheap bookkeeping steps that
    /// reserved a slot but did NOT perform credential verification —
    /// e.g. `/api/auth/passkey/login/start` which only builds a WebAuthn
    /// challenge.
    ///
    /// Critical: `record_success` would `state.map.remove(&ip_key)`, which
    /// wipes the per-IP failure counter and lockout window. An attacker
    /// who can hit a "neutral" endpoint can otherwise climb toward the
    /// lockout threshold on the real verify endpoint, then call the
    /// neutral endpoint once to wipe the counter and resume hammering.
    /// `release_slot` decrements `in_flight` only — `failures` and
    /// `locked_until` are preserved.
    pub async fn release_slot(mut self) {
        self.consumed = true;
        let mut state = self.inner.write().await;
        if let Some(entry) = state.map.get_mut(&self.ip_key) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
        }
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
            Ok(mut state) => apply_failure(&mut state, self.ip_key, self.backoff_cap),
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
///
/// itr#233: `locked_until` is only ever advanced from
/// `max(now, locked_until)` — i.e. only when the previous lockout has
/// already expired (`now >= locked_until`). Before this fix the deadline
/// was unconditionally recomputed as `now + backoff_for(failures)` on
/// *every* failure, so an attacker who keeps probing (or whose connection
/// keeps getting torn down and fail-closed via `Drop`) during an active
/// lockout window would push the deadline further into the future on each
/// attempt — `now` keeps advancing even though `backoff_for` itself is
/// capped at 30s. A legitimate user sharing the same NAT'd IP could then
/// face a soft-lock that never actually clears no matter how long they
/// wait. Gating the recompute on the previous lockout having expired still
/// lets `failures` accumulate (so the schedule reflects the full attempt
/// count once the lockout does lapse), but a probe that lands *inside* an
/// active lockout can no longer extend it.
///
/// itr#499: under a raised in-flight cap (itr#243, `max_in_flight > 1`)
/// several distinct guards can be admitted *before* the lockout exists and
/// then fail near-simultaneously. The first fail sets
/// `locked_until = now + backoff_for(1)`; the siblings land inside that
/// fresh window, so the `now >= locked_until` recompute above is suppressed
/// and the lockout stays pinned at the 1-failure rung even though the burst
/// was larger. To fix that without reintroducing the itr#233 bug we advance
/// the deadline *monotonically* — `max(locked_until, now + backoff_for(...))`,
/// which never shortens an existing lockout — but **only while another guard
/// from the same burst is still in flight** (`in_flight > 0` before we
/// release this one). That gate is what keeps this honest: `try_begin_attempt`
/// refuses to admit a new guard while an IP is locked out, so the only
/// failures that can reach here mid-lockout are the tail of the original
/// concurrent burst. Once that burst fully drains (`in_flight == 0`) no
/// further guard can be admitted until the lockout lapses, so a lone probe
/// re-hitting an already-locked entry (the itr#233 case, and what
/// `record_failure_does_not_extend_active_lockout` drives directly with
/// `in_flight == 0`) can never push the deadline out.
fn apply_failure(state: &mut ThrottleInner, ip_key: IpAddr, backoff_cap: Duration) {
    let now = Instant::now();
    if let Some(entry) = state.map.get_mut(&ip_key) {
        entry.failures = entry.failures.saturating_add(1);
        if now >= entry.locked_until {
            // Previous lockout (if any) has lapsed — open a fresh window
            // from `now` reflecting the accumulated failure count.
            entry.locked_until = now + backoff_for(entry.failures, backoff_cap);
        } else if entry.in_flight > 0 {
            // Still inside an active lockout, but this is a genuine sibling
            // failure from a concurrent burst (another guard is outstanding).
            // Advance the deadline to reflect the higher rung, never shortening
            // it (monotonic — preserves itr#233).
            entry.locked_until = entry
                .locked_until
                .max(now + backoff_for(entry.failures, backoff_cap));
        }
        entry.in_flight = entry.in_flight.saturating_sub(1);
        entry.last_seen = now;
    }
}

/// Backoff schedule: `min(cap, 250ms * 2^(N-1))`.
///
/// `cap` used to be a hardcoded 30s; itr#246 made it a caller-supplied
/// parameter (see [`DEFAULT_BACKOFF_CAP`] / [`BACKOFF_CAP_SECS_ENV`]) so a
/// deployment can raise it once it has real UX telemetry to justify a
/// larger value, without recompiling.
fn backoff_for(failures: u32, cap: Duration) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
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
// Peek throttle
// ---------------------------------------------------------------------------

/// Requests allowed per IP per [`PEEK_WINDOW`]. Generous — this throttle
/// exists to stop scripted hammering of read-only discovery endpoints, not
/// to rate-limit normal UI polling.
const PEEK_LIMIT: u32 = 60;
/// Fixed-window size for [`PeekThrottle`].
const PEEK_WINDOW: Duration = Duration::from_secs(60);
/// Hard cap on distinct (normalized) IPs tracked by [`PeekThrottle`]. Same
/// bound and rationale as [`MAX_THROTTLE_ENTRIES`] — bounds memory under an
/// IP-spray attack.
const MAX_PEEK_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct PeekState {
    /// Requests counted in the current window.
    count: u32,
    /// Start of the current fixed window.
    window_start: Instant,
}

#[derive(Debug, Default)]
struct PeekThrottleInner {
    map: HashMap<IpAddr, PeekState>,
}

/// Per-IP rate limit for unauthenticated *read-only* discovery endpoints —
/// today `/api/auth/status` and `/api/auth/profile` (itr#317).
///
/// **Why not reuse [`LoginThrottle`].** `LoginThrottle` models credential
/// *attempts*: it tracks failures with exponential backoff, reserves an
/// in-flight slot per verify, and wipes its counter on a successful login.
/// None of that applies here — these endpoints never verify a credential,
/// so there is no "failure" to back off from and no "success" to clear a
/// counter that was never incremented. Sharing the budget would also let
/// unrelated traffic classes interfere with each other: a script hammering
/// `/api/auth/status` could push a legitimate operator's IP toward a login
/// lockout, and conversely a string of failed logins would tighten the
/// budget for a same-IP discovery probe that has nothing to do with the
/// failed attempts. `PeekThrottle` is instead a plain fixed-window counter
/// — `PEEK_LIMIT` requests per `PEEK_WINDOW`, reset wholesale each window —
/// with its own bounded map and its own budget.
///
/// **Fail-open on map exhaustion**, unlike `LoginThrottle`'s fail-closed
/// stance: `LoginThrottle`'s bound is a security control against credential
/// brute force, so refusing an unbounded-map insert is the safe default.
/// `PeekThrottle` only guards read-only discovery data (whether a password
/// is set, which auth profile is active) — the worst case of a missed rate
/// limit is more scripted probing of information that isn't secret,
/// against the (already itself bounded) risk of an operator on a full map
/// being wrongly 429'd.
#[derive(Debug, Clone, Default)]
pub struct PeekThrottle {
    inner: Arc<RwLock<PeekThrottleInner>>,
}

impl PeekThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically check-and-increment the per-IP counter for the current
    /// window. Returns `true` when the request is under budget (and counts
    /// it), `false` when the IP should be refused with `429`.
    pub async fn allow(&self, ip: IpAddr) -> bool {
        let key = normalize_ip(ip);
        let mut state = self.inner.write().await;
        let now = Instant::now();

        if let Some(entry) = state.map.get_mut(&key) {
            if now.saturating_duration_since(entry.window_start) >= PEEK_WINDOW {
                entry.count = 0;
                entry.window_start = now;
            }
            if entry.count >= PEEK_LIMIT {
                return false;
            }
            entry.count += 1;
            return true;
        }

        if !ensure_room_for_new_peek_entry(&mut state, now) {
            // Cap full and nothing evictable (every peek entry is always
            // evictable once its window lapses, so this is effectively
            // unreachable) — fail open, see the type-level docs above.
            return true;
        }
        state.map.insert(
            key,
            PeekState {
                count: 1,
                window_start: now,
            },
        );
        true
    }
}

/// Make room for one more entry in `state.map` without exceeding
/// [`MAX_PEEK_ENTRIES`]. Sweeps windows that have already lapsed first
/// (cheap, O(N)); if that isn't enough, evicts the single oldest entry by
/// `window_start` to make room for the caller's insert. Returns `false`
/// only if the map is still at cap with nothing to evict (shouldn't happen
/// — every entry has a bounded window and is always eventually evictable).
fn ensure_room_for_new_peek_entry(state: &mut PeekThrottleInner, now: Instant) -> bool {
    if state.map.len() < MAX_PEEK_ENTRIES {
        return true;
    }
    state
        .map
        .retain(|_, s| now.saturating_duration_since(s.window_start) < PEEK_WINDOW);
    if state.map.len() < MAX_PEEK_ENTRIES {
        return true;
    }
    let oldest = state
        .map
        .iter()
        .min_by_key(|(_, s)| s.window_start)
        .map(|(k, _)| *k);
    match oldest {
        Some(key) => {
            state.map.remove(&key);
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// Build a PHC string with caller-chosen algorithm/params, bypassing
    /// `argon2_instance`'s enforced minimums, so tests can construct a
    /// deliberately-weakened hash the way a tampered DB row would look.
    fn hash_with_params(password: &str, algorithm: Algorithm, params: Params) -> String {
        let mut salt_bytes = [0u8; 16];
        rand::rngs::OsRng.try_fill_bytes(&mut salt_bytes).unwrap();
        let salt = SaltString::encode_b64(&salt_bytes).unwrap();
        let argon2 = Argon2::new(algorithm, Version::V0x13, params);
        argon2
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
            .to_string()
    }

    #[test]
    fn argon2_roundtrip() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &phc));
        assert!(!verify_password("wrong password", &phc));
        assert!(!verify_password("", &phc));
    }

    /// itr#232 acceptance: a PHC string using `argon2i` instead of
    /// `argon2id` must be rejected by `verify_password`, even against the
    /// correct password — otherwise a tampered DB row can downgrade the
    /// algorithm undetected.
    #[test]
    fn rejects_argon2i_algorithm() {
        let password = "correct horse battery staple";
        let params = Params::new(MIN_M_COST, MIN_T_COST, MIN_P_COST, None).unwrap();
        let phc = hash_with_params(password, Algorithm::Argon2i, params);
        assert!(
            !verify_password(password, &phc),
            "argon2i PHC string must be rejected regardless of password correctness"
        );
    }

    /// itr#502: a PHC string with `m_cost` below the enforced minimum must
    /// NOT permanently lock out the correct password (the one-way ratchet
    /// #502 closes). The correct password verifies against the hash's
    /// embedded params and the outcome flags that a rehash is warranted; a
    /// wrong password against the same below-floor hash still fails, so no
    /// floor state leaks through the boolean.
    #[test]
    fn below_floor_m_cost_verifies_and_flags_rehash() {
        let password = "correct horse battery staple";
        let weak_m_cost = MIN_M_COST - 1;
        let params = Params::new(weak_m_cost, MIN_T_COST, MIN_P_COST, None).unwrap();
        let phc = hash_with_params(password, Algorithm::Argon2id, params);

        // Correct password: closes the lockout ratchet — returns true.
        assert!(
            verify_password(password, &phc),
            "correct password against a below-floor m_cost hash must still verify (itr#502)"
        );
        assert_eq!(
            verify_password_with_migration(password, &phc),
            PasswordVerification::OkRehashNeeded,
            "a correct password against a below-floor hash must flag a rehash, not lock out"
        );

        // Wrong password against the same below-floor hash still fails.
        assert!(!verify_password("wrong password", &phc));
        assert_eq!(
            verify_password_with_migration("wrong password", &phc),
            PasswordVerification::Failed
        );
    }

    /// Sibling check for `t_cost` below minimum — same itr#502 migration
    /// semantics as m_cost. Also pins that p_cost's floor tracks the crate's
    /// own `Params::MIN_P_COST`, so there's no silent gap below it.
    #[test]
    fn below_floor_t_cost_verifies_and_flags_rehash() {
        let password = "correct horse battery staple";

        let low_t = Params::new(MIN_M_COST, MIN_T_COST - 1, MIN_P_COST, None).unwrap();
        let phc_t = hash_with_params(password, Algorithm::Argon2id, low_t);
        assert!(
            verify_password(password, &phc_t),
            "correct password against a below-floor t_cost hash must still verify (itr#502)"
        );
        assert_eq!(
            verify_password_with_migration(password, &phc_t),
            PasswordVerification::OkRehashNeeded
        );
        assert!(!verify_password("wrong password", &phc_t));

        // p_cost's floor is already `Params::MIN_P_COST` (1) at the crate
        // level, so there's no way to construct a PHC string below our
        // MIN_P_COST=1 threshold — this documents that MIN_P_COST tracks
        // the crate's own floor rather than leaving a silent gap.
        assert_eq!(MIN_P_COST, argon2::Params::MIN_P_COST);
    }

    /// A hash produced at exactly the minimum thresholds must verify cleanly
    /// as `Ok` (no spurious rehash flag) — the floor check must not be
    /// off-by-one against a legitimate at-floor hash.
    #[test]
    fn accepts_hash_at_exact_minimum_params() {
        let password = "correct horse battery staple";
        let params = Params::new(MIN_M_COST, MIN_T_COST, MIN_P_COST, None).unwrap();
        let phc = hash_with_params(password, Algorithm::Argon2id, params);
        assert!(verify_password(password, &phc));
        assert_eq!(
            verify_password_with_migration(password, &phc),
            PasswordVerification::Ok
        );
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

    /// itr#244: sibling to `parallel_attempts_one_in_flight` that holds the
    /// winner's guard across an actual elapsed-time sleep (under paused
    /// tokio time) instead of a `yield_now` + `Notify` handshake. That test
    /// forces overlap cooperatively; this one exercises the full
    /// check-then-act window the original race was about — the winner is
    /// genuinely "away" doing slow work (e.g. verifying a password hash)
    /// while real wall-clock time (paused, but still advancing per
    /// `tokio::time::sleep`) passes and racers arrive. Belt-and-suspenders
    /// coverage, not a replacement.
    #[tokio::test(start_paused = true)]
    async fn parallel_attempts_one_in_flight_held_across_sleep() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();

        // Winner grabs the slot, then holds it across a paused-clock sleep
        // on a separate task, standing in for slow work (e.g. Argon2
        // verification) done while the reservation is outstanding.
        let winner = throttle
            .try_begin_attempt(ip)
            .await
            .expect("winner should grab the slot");
        let hold = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            winner
        });

        // Racers each sleep a shorter interval so, under paused time, they
        // wake and attempt while the winner's hold task is still asleep —
        // i.e. genuinely mid-hold, not after the guard has been released.
        let handles: Vec<_> = (0..9)
            .map(|_| {
                let t = throttle.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    t.try_begin_attempt(ip).await
                })
            })
            .collect();

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
            "racers should all be throttled while the original guard is held across the sleep"
        );
        assert_eq!(throttled, 9, "all 9 racers should report throttled");

        let winner = hold.await.unwrap();
        winner.record_failure().await;
    }

    /// itr#243: `parse_max_in_flight` must default to
    /// `DEFAULT_MAX_IN_FLIGHT_PER_IP` (1) on anything that isn't a positive
    /// integer, and otherwise take the configured value.
    #[test]
    fn parse_max_in_flight_defaults_and_overrides() {
        assert_eq!(parse_max_in_flight(None), DEFAULT_MAX_IN_FLIGHT_PER_IP);
        assert_eq!(parse_max_in_flight(Some("")), DEFAULT_MAX_IN_FLIGHT_PER_IP);
        assert_eq!(
            parse_max_in_flight(Some("not-a-number")),
            DEFAULT_MAX_IN_FLIGHT_PER_IP
        );
        assert_eq!(parse_max_in_flight(Some("0")), DEFAULT_MAX_IN_FLIGHT_PER_IP);
        assert_eq!(
            parse_max_in_flight(Some("-3")),
            DEFAULT_MAX_IN_FLIGHT_PER_IP
        );
        assert_eq!(parse_max_in_flight(Some("5")), 5);
        assert_eq!(parse_max_in_flight(Some("  7  ")), 7);
    }

    /// itr#243 acceptance: `LoginThrottle::new()` (the env-var path) keeps
    /// the default cap at 1 when the override is unset, and a throttle
    /// built with an explicit override actually admits more concurrent
    /// in-flight attempts per IP — proving the knob takes effect at
    /// runtime rather than merely existing as an unused field.
    #[tokio::test]
    async fn configured_max_in_flight_raises_the_cap() {
        // Default path: unset env var (assumed unset in the test process)
        // still throttles the 2nd concurrent attempt, same as before this
        // ticket.
        let default_throttle = LoginThrottle::new();
        let default_ip: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();
        let _first = default_throttle
            .try_begin_attempt(default_ip)
            .await
            .expect("first attempt allowed under default cap");
        let err = default_throttle
            .try_begin_attempt(default_ip)
            .await
            .unwrap_err();
        assert!(
            !err.allowed,
            "default cap must remain 1 — second concurrent attempt should be throttled"
        );

        // Configured path: an explicit cap of 3 admits up to 3 concurrent
        // in-flight attempts for the same IP before throttling the 4th.
        let configured = LoginThrottle::with_max_in_flight(3);
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 3).into();
        let g1 = configured
            .try_begin_attempt(ip)
            .await
            .expect("1st attempt allowed under configured cap of 3");
        let g2 = configured
            .try_begin_attempt(ip)
            .await
            .expect("2nd attempt allowed under configured cap of 3");
        let g3 = configured
            .try_begin_attempt(ip)
            .await
            .expect("3rd attempt allowed under configured cap of 3");
        let err = configured.try_begin_attempt(ip).await.unwrap_err();
        assert!(!err.allowed, "4th concurrent attempt must be throttled");

        g1.record_success().await;
        g2.record_success().await;
        g3.record_success().await;
    }

    /// itr#498 regression: with `max_in_flight > 1`, one sibling's
    /// `record_success` must release ONLY its own reservation, not wipe the
    /// whole per-IP entry (which would erase the in-flight counts of siblings
    /// still verifying). Under the old `state.map.remove(&ip_key)` the shared
    /// counter was zeroed while g2/g3 were still outstanding, so fresh attempts
    /// stacked on top and admitted concurrency blew past the configured cap.
    #[tokio::test]
    async fn record_success_releases_only_own_slot_respecting_cap() {
        let throttle = LoginThrottle::with_max_in_flight(3);
        let ip: IpAddr = Ipv4Addr::new(203, 0, 113, 9).into();
        let key = normalize_ip(ip);

        // Fill the cap: three concurrent verifies outstanding.
        let g1 = throttle.try_begin_attempt(ip).await.expect("g1 admitted");
        let g2 = throttle.try_begin_attempt(ip).await.expect("g2 admitted");
        let g3 = throttle.try_begin_attempt(ip).await.expect("g3 admitted");

        // At the cap — a 4th concurrent attempt must be refused.
        assert!(
            throttle.try_begin_attempt(ip).await.is_err(),
            "cap=3 already full (g1,g2,g3) — 4th concurrent attempt must be rejected"
        );

        // One sibling succeeds while g2 and g3 are STILL in flight. The old
        // `map.remove` wiped g2/g3's shared in_flight counter here.
        g1.record_success().await;

        // Exactly one slot should now be free — the true outstanding count is
        // 2 (g2, g3), so g4 is admitted and the counter reads 3.
        let g4 = throttle
            .try_begin_attempt(ip)
            .await
            .expect("one slot freed by g1's success — g4 must be admitted");

        {
            let state = throttle.inner.read().await;
            let entry = state
                .map
                .get(&key)
                .expect("entry present with siblings in flight");
            assert_eq!(
                entry.in_flight, 3,
                "in_flight must equal the true outstanding guards (g2,g3,g4), never exceeding the cap"
            );
        }

        // Back at the cap (g2,g3,g4) — a 5th concurrent attempt must be
        // rejected. Under the buggy code the entry was gone, so this would
        // wrongly succeed and admitted concurrency would exceed the cap of 3.
        assert!(
            throttle.try_begin_attempt(ip).await.is_err(),
            "cap=3 full again (g2,g3,g4) — 5th concurrent attempt must be rejected"
        );

        // Mixed outcome: g2 fails while g3/g4 are still in flight, then both
        // succeed. The entry must GC to empty once no guard is outstanding.
        g2.record_failure().await;
        g3.record_success().await;
        g4.record_success().await;

        let state = throttle.inner.read().await;
        assert!(
            !state.map.contains_key(&key),
            "entry must be GC'd once no guard is outstanding and the lockout is clear"
        );
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

    /// itr#233 acceptance: calling `record_failure` 100 times while a
    /// lockout is already active must not extend `locked_until` beyond
    /// `schedule(failures+100)` capped at 30s. In practice
    /// `try_begin_attempt` refuses to hand out a guard while an IP is
    /// locked out (a caller can't reach `AttemptGuard::record_failure`
    /// mid-lockout through the public API), so this drives the shared
    /// `apply_failure` helper directly — the same code path
    /// `record_failure` and the fail-closed `Drop` both fall through to —
    /// to isolate exactly the invariant the ticket describes: once locked
    /// out, further failures must not push the deadline out further.
    #[tokio::test(start_paused = true)]
    async fn record_failure_does_not_extend_active_lockout() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let key = normalize_ip(ip);

        // One failure puts the IP into an active lockout.
        throttle
            .try_begin_attempt(ip)
            .await
            .expect("first attempt should be allowed")
            .record_failure()
            .await;
        let locked_until_before = {
            let state = throttle.inner.read().await;
            state
                .map
                .get(&key)
                .expect("entry should exist")
                .locked_until
        };
        assert!(
            Instant::now() < locked_until_before,
            "sanity: IP should be locked out immediately after the first failure"
        );

        // Hammer the same failure path 100 more times while still inside
        // that active lockout window.
        for _ in 0..100 {
            let mut state = throttle.inner.write().await;
            apply_failure(&mut state, key, DEFAULT_BACKOFF_CAP);
        }

        let state = throttle.inner.read().await;
        let entry = state.map.get(&key).expect("entry should still exist");
        assert_eq!(
            entry.failures, 101,
            "failure count should still accumulate during lockout"
        );
        assert_eq!(
            entry.locked_until, locked_until_before,
            "locked_until must not move while the previous lockout is still \
             active, even after 100 more failures"
        );
        assert!(
            entry.locked_until <= locked_until_before + Duration::from_secs(30),
            "lockout must never exceed schedule(failures) capped at 30s"
        );
    }

    /// itr#499 acceptance: under a raised in-flight cap (itr#243), several
    /// guards can be admitted before any lockout exists and then fail
    /// near-simultaneously. The first failure opens a window at
    /// `backoff_for(1)`; the siblings land *inside* that fresh window. The
    /// lockout must still advance to reflect the full burst size —
    /// `backoff_for(3)` for three concurrent failures — not stay pinned at
    /// the 1-failure rung. This complements
    /// `record_failure_does_not_extend_active_lockout`: a lone probe with no
    /// sibling in flight (`in_flight == 0`) still can't extend the lockout,
    /// but a genuine concurrent burst does.
    #[tokio::test(start_paused = true)]
    async fn concurrent_burst_advances_lockout_to_full_rung() {
        let throttle = LoginThrottle::with_max_in_flight(3);
        let ip: IpAddr = Ipv4Addr::new(198, 51, 100, 7).into();
        let key = normalize_ip(ip);

        // Fill the cap: three concurrent verifies admitted before any lockout
        // exists, so none is rejected by `locked_until`.
        let g1 = throttle.try_begin_attempt(ip).await.expect("g1 admitted");
        let g2 = throttle.try_begin_attempt(ip).await.expect("g2 admitted");
        let g3 = throttle.try_begin_attempt(ip).await.expect("g3 admitted");

        // All three fail near-simultaneously (the paused clock keeps `now`
        // fixed, so g2 and g3 land inside g1's fresh lockout window).
        g1.record_failure().await;
        g2.record_failure().await;
        g3.record_failure().await;

        let state = throttle.inner.read().await;
        let entry = state.map.get(&key).expect("entry should exist");
        assert_eq!(
            entry.failures, 3,
            "all three concurrent failures should have accumulated"
        );
        assert_eq!(
            entry.in_flight, 0,
            "all three guards should have been released"
        );

        let remaining = entry.locked_until - Instant::now();
        assert_eq!(
            remaining,
            backoff_for(3, DEFAULT_BACKOFF_CAP),
            "lockout must reflect the full 3-failure burst, not the 1-failure rung"
        );
        assert_ne!(
            remaining,
            backoff_for(1, DEFAULT_BACKOFF_CAP),
            "lockout must not stay pinned at backoff_for(1) under a raised in-flight cap"
        );
    }

    #[test]
    fn backoff_schedule_values() {
        // Spot-check a few rungs of the ladder at the default 30s cap.
        let cap = DEFAULT_BACKOFF_CAP;
        assert_eq!(backoff_for(1, cap), Duration::from_millis(250));
        assert_eq!(backoff_for(2, cap), Duration::from_millis(500));
        assert_eq!(backoff_for(3, cap), Duration::from_secs(1));
        assert_eq!(backoff_for(4, cap), Duration::from_secs(2));
        assert_eq!(backoff_for(5, cap), Duration::from_secs(4));
        assert_eq!(backoff_for(6, cap), Duration::from_secs(8));
        assert_eq!(backoff_for(7, cap), Duration::from_secs(16));
        assert_eq!(backoff_for(8, cap), Duration::from_secs(30));
        assert_eq!(backoff_for(20, cap), Duration::from_secs(30));
        assert_eq!(backoff_for(u32::MAX, cap), Duration::from_secs(30));
    }

    /// itr#246: `backoff_for` must respect a caller-supplied cap instead of
    /// the hardcoded 30s ceiling it used to have — proving the schedule
    /// keeps doubling past the old 30s point when a deployment configures
    /// a longer cap (e.g. 5 minutes), and still clamps at the new cap.
    #[test]
    fn backoff_schedule_respects_configured_cap() {
        let cap = Duration::from_secs(300); // 5 minutes.
        // Rungs below the old 30s cap are unaffected.
        assert_eq!(backoff_for(1, cap), Duration::from_millis(250));
        assert_eq!(backoff_for(8, cap), Duration::from_secs(32));
        // Past the old 30s cap, the schedule now keeps climbing instead of
        // clamping at 30s.
        assert_eq!(backoff_for(9, cap), Duration::from_secs(64));
        assert_eq!(backoff_for(10, cap), Duration::from_secs(128));
        // 250ms * 2^10 = 256s, still under the 300s cap.
        assert_eq!(backoff_for(11, cap), Duration::from_secs(256));
        // 250ms * 2^11 = 512s, clamps at the configured 300s cap.
        assert_eq!(backoff_for(12, cap), cap);
        assert_eq!(backoff_for(u32::MAX, cap), cap);
    }

    /// itr#246: `parse_backoff_cap_secs` must default to
    /// `DEFAULT_BACKOFF_CAP` (30s) on anything that isn't a positive
    /// integer, and otherwise take the configured value — same shape as
    /// `parse_max_in_flight` (itr#243).
    #[test]
    fn parse_backoff_cap_secs_defaults_and_overrides() {
        assert_eq!(parse_backoff_cap_secs(None), DEFAULT_BACKOFF_CAP);
        assert_eq!(parse_backoff_cap_secs(Some("")), DEFAULT_BACKOFF_CAP);
        assert_eq!(
            parse_backoff_cap_secs(Some("not-a-number")),
            DEFAULT_BACKOFF_CAP
        );
        assert_eq!(parse_backoff_cap_secs(Some("0")), DEFAULT_BACKOFF_CAP);
        assert_eq!(parse_backoff_cap_secs(Some("-3")), DEFAULT_BACKOFF_CAP);
        assert_eq!(
            parse_backoff_cap_secs(Some("300")),
            Duration::from_secs(300)
        );
        assert_eq!(
            parse_backoff_cap_secs(Some("  120  ")),
            Duration::from_secs(120)
        );
    }

    /// Drives `count` failures against `ip` on `throttle`, using the
    /// paused clock to jump past any active lockout between attempts so
    /// `failures` actually accumulates to `count` instead of being
    /// rejected by `locked_until`. Returns the final `locked_until`
    /// distance from "now" once the last failure lands.
    async fn drive_failures_and_measure_lockout(
        throttle: &LoginThrottle,
        ip: IpAddr,
        count: u32,
    ) -> Duration {
        let key = normalize_ip(ip);
        for _ in 0..count {
            let locked_until = {
                let state = throttle.inner.read().await;
                state.map.get(&key).map(|s| s.locked_until)
            };
            if let Some(locked_until) = locked_until {
                let now = Instant::now();
                if locked_until > now {
                    tokio::time::advance(locked_until - now).await;
                }
            }
            throttle
                .try_begin_attempt(ip)
                .await
                .expect("attempt allowed once any previous lockout has been waited out")
                .record_failure()
                .await;
        }
        let state = throttle.inner.read().await;
        let entry = state.map.get(&key).expect("entry should exist");
        entry.locked_until - Instant::now()
    }

    /// itr#246 acceptance: `LoginThrottle::new()` (the env-var path) keeps
    /// the default backoff cap at 30s when the override is unset (per the
    /// sprint's own non-goal — no UX-measured hardcoded bump), and a
    /// throttle built with an explicit override actually produces a
    /// longer lockout — proving the knob takes effect at runtime rather
    /// than merely existing as an unused field.
    #[tokio::test(start_paused = true)]
    async fn configured_backoff_cap_raises_the_lockout() {
        // Default path: unset env var (assumed unset in the test process)
        // still caps at 30s even after 12 failures (well past the 8th
        // rung where the old hardcoded schedule saturated).
        let default_throttle = LoginThrottle::new();
        let default_ip: IpAddr = Ipv4Addr::new(10, 0, 0, 4).into();
        let default_lockout =
            drive_failures_and_measure_lockout(&default_throttle, default_ip, 12).await;
        assert!(
            default_lockout <= DEFAULT_BACKOFF_CAP,
            "default cap must remain 30s, got {default_lockout:?}"
        );

        // Configured path: an explicit 5-minute cap lets the same number
        // of failures produce a lockout longer than the old 30s ceiling.
        let configured = LoginThrottle::with_backoff_cap(Duration::from_secs(300));
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 5).into();
        let configured_lockout = drive_failures_and_measure_lockout(&configured, ip, 12).await;
        assert!(
            configured_lockout > DEFAULT_BACKOFF_CAP,
            "configured 5-minute cap must allow lockouts longer than the old 30s ceiling, got {configured_lockout:?}"
        );
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

    // ── PeekThrottle (itr#317) ─────────────────────────────────────────

    #[tokio::test]
    async fn peek_throttle_allows_under_budget() {
        let throttle = PeekThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        for i in 0..PEEK_LIMIT {
            assert!(
                throttle.allow(ip).await,
                "request {i} should be allowed (limit is {PEEK_LIMIT})"
            );
        }
    }

    #[tokio::test]
    async fn peek_throttle_denies_above_budget() {
        let throttle = PeekThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        for _ in 0..PEEK_LIMIT {
            assert!(throttle.allow(ip).await);
        }
        // One more over budget must be denied.
        assert!(
            !throttle.allow(ip).await,
            "request past PEEK_LIMIT should be denied"
        );
        assert!(
            !throttle.allow(ip).await,
            "throttle should keep denying while still in the same window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn peek_throttle_resets_after_window() {
        let throttle = PeekThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        for _ in 0..PEEK_LIMIT {
            assert!(throttle.allow(ip).await);
        }
        assert!(!throttle.allow(ip).await);

        tokio::time::advance(PEEK_WINDOW + Duration::from_millis(10)).await;
        assert!(
            throttle.allow(ip).await,
            "a new window should reset the counter"
        );
    }

    #[tokio::test]
    async fn peek_throttle_per_ip_isolation() {
        let throttle = PeekThrottle::new();
        let hot: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let quiet: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();

        // Exhaust `hot`'s budget.
        for _ in 0..PEEK_LIMIT {
            assert!(throttle.allow(hot).await);
        }
        assert!(!throttle.allow(hot).await, "hot IP should now be denied");

        // `quiet` must be unaffected — separate budget entirely.
        assert!(
            throttle.allow(quiet).await,
            "a different IP must have its own independent budget"
        );
    }
}
