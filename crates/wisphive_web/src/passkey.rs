//! WebAuthn passkey backend for the web UI (itr#311 / PR-4 of #219).
//!
//! Owns three concerns the [`crate::lib::build_router`] handlers consume:
//!
//! 1. **[`webauthn_for`]** — a lazy `OnceCell`-backed cache of
//!    `webauthn-rs::Webauthn` instances, keyed by `(rp_id, rp_origin)`.
//!    Building a `Webauthn` parses URLs, picks COSE algorithms, and warms
//!    the OpenSSL stack — none of which we want to repeat per request.
//!    Under Enterprise the cache holds exactly one entry (the static RP
//!    ID + origin); under LocalLAN it holds at most one (loopback only —
//!    `AuthPolicy::rp_id_for_origin` returns `None` for LAN-IP origins,
//!    so the cache is never queried for them).
//!
//! 2. **[`ChallengeStore`]** — opaque-session-id keyed in-memory map of
//!    pending registration / authentication ceremonies, with TTL eviction
//!    driven by [`AuthPolicy::challenge_ttl`]. `take(session_id)` removes
//!    on read so single-use is enforced by the data structure rather than
//!    the caller. A background tokio task ticks `evict_expired` every 60s
//!    so a hostile client can't grow the map without bound by starting
//!    challenges they never finish.
//!
//! 3. **Helper conversions** — credential ID encoding / decoding,
//!    `Passkey ↔ DiscoverableKey` conversion, and the small surface the
//!    handlers need to avoid leaking `webauthn-rs` types into
//!    `crate::lib`.
//!
//! ## Design notes from the /alignment session (2026-05-16) and the
//! itr#310 review handoff
//!
//! - `WebauthnBuilder::timeout(policy.challenge_ttl)` is applied at
//!   builder time, BEFORE `.build()`. Both register AND authenticate
//!   ceremonies inherit it because webauthn-rs uses the value on the
//!   server-side `RegistrationState` / `AuthenticationState` that survives
//!   the round-trip — there's no per-call override. Tests below assert
//!   the cache returns the same Arc for the same key (so the TTL is
//!   consistent across calls) and that the value is stored on the
//!   builder.
//! - `rp_origin` for LocalLAN MUST be `https://localhost:<port>` even
//!   when the request arrived via `127.0.0.1` — `AuthPolicy::rp_id_for_origin`
//!   collapses loopback hosts to RP ID `"localhost"`, and webauthn-rs
//!   rejects ceremonies whose `rp_origin` doesn't match. The caller
//!   (handlers in `crate::lib`) is responsible for building the correct
//!   `rp_origin` from the resolved RP ID, not the raw request URL.
//! - **TODO(itr#270)**: Enterprise `rp_origin` is derived as
//!   `https://{rp_id}` in `wisphive_cli` today. When itr#270 lands and
//!   operators provide `--tls-cert`, the cert's primary SAN may not
//!   match this derivation — revisit then. The `webauthn_for` cache key
//!   includes the full `rp_origin` (`Url`), so swapping origins doesn't
//!   alias incorrectly with the old entry; the old entry just becomes
//!   dead weight.
//! - **Resident keys (discoverable credentials)**: webauthn-rs 0.5's
//!   `start_passkey_registration` hard-codes `require_resident_key(false)`
//!   AND `UserVerificationPolicy::Required`. Modern authenticators
//!   (Apple, Google, Windows Hello, 1Password) create resident
//!   credentials regardless because they lack persistent
//!   non-discoverable storage, so the flag's value is moot in practice —
//!   but it's a deviation from the locked design and noted here in case
//!   we ever ship to security-key-only deployments.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use rand::TryRngCore;
use tokio::sync::{Mutex, OnceCell};
use tokio::time::Instant;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, PasskeyAuthentication, PasskeyRegistration, Url,
};
use webauthn_rs::{Webauthn, WebauthnBuilder};

use crate::auth_profile::{AuthPolicy, RpId};

// ---------------------------------------------------------------------------
// Webauthn instance cache
// ---------------------------------------------------------------------------

/// Cache of `Arc<Webauthn>` instances, keyed by `(rp_id, rp_origin)`.
///
/// Lives behind a `OnceCell` so the global map itself is constructed
/// lazily on first reference; behind a `Mutex` so concurrent first-touches
/// for distinct keys serialize cleanly. The inner cache is unlikely to
/// see contention in steady state (one entry under Enterprise, at most
/// one under LocalLAN) so a plain `Mutex` is fine — no need for a
/// `RwLock` dance.
///
/// We deliberately don't share the OnceCell across processes / restarts;
/// `Webauthn` carries no persistent state (challenges live in
/// [`ChallengeStore`]) so a fresh build per process is correct.
static WEBAUTHN_CACHE: OnceCell<Mutex<HashMap<WebauthnCacheKey, Arc<Webauthn>>>> =
    OnceCell::const_new();

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct WebauthnCacheKey {
    rp_id: String,
    rp_origin: Url,
    /// Include the challenge TTL in the cache key so a test (or an
    /// operator who somehow swaps the policy at runtime) can't accidentally
    /// reuse a stale `Webauthn` whose `WebauthnBuilder::timeout` was set
    /// to the old value. In production this is invariant — the policy
    /// is frozen at startup — so the cache still degenerates to "one
    /// entry per profile".
    challenge_ttl_ms: u64,
}

/// Resolve (and lazily build) the `Arc<Webauthn>` for the given
/// `(rp_id, rp_origin)` pair under the active [`AuthPolicy`].
///
/// `policy` is read at build time for `challenge_ttl` only — every
/// downstream ceremony inherits the timeout. Other policy bits
/// (`uv_requirement`, `passkey_required`, etc.) are not represented on
/// the builder because webauthn-rs's higher-level `start_passkey_*` APIs
/// either hardcode them (UV) or don't take them at all. See the
/// module-level docstring for the rationale.
///
/// The cache key is `(rp_id_string, rp_origin)` — under Enterprise this
/// is a single entry for the lifetime of the process. Under LocalLAN
/// the only legal RP ID is `"localhost"` and the only legal rp_origin
/// is `https://localhost:<port>` (loopback), so at most one entry.
///
/// TODO(itr#270): When operators ship their own TLS cert, the Enterprise
/// `rp_origin` derived as `https://{rp_id}` in
/// `wisphive_cli::resolve_auth_profile` may not match the cert's primary
/// SAN. Revisit the rp_origin source then; this cache will pick up the
/// new key naturally (the old entry just becomes orphan memory of size
/// O(1)).
pub async fn webauthn_for(
    rp_id: &RpId,
    rp_origin: &Url,
    policy: &AuthPolicy,
) -> anyhow::Result<Arc<Webauthn>> {
    let cache = WEBAUTHN_CACHE
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;
    let key = WebauthnCacheKey {
        rp_id: rp_id.as_str().to_string(),
        rp_origin: rp_origin.clone(),
        challenge_ttl_ms: policy.challenge_ttl.as_millis() as u64,
    };

    let mut guard = cache.lock().await;
    if let Some(existing) = guard.get(&key) {
        return Ok(existing.clone());
    }

    // First touch for this key — build a fresh Webauthn. `WebauthnBuilder::new`
    // validates that the rp_id is a non-empty DNS-safe string and that
    // `rp_origin` has a host; both invariants are upheld by `AuthPolicy`
    // construction, so this should never fail in practice.
    let builder = WebauthnBuilder::new(rp_id.as_str(), rp_origin)
        .map_err(|e| anyhow::anyhow!("WebauthnBuilder::new failed for rp_id={rp_id}: {e}"))?
        .rp_name("Wisphive")
        .timeout(policy.challenge_ttl);

    let webauthn = builder
        .build()
        .map_err(|e| anyhow::anyhow!("WebauthnBuilder::build failed for rp_id={rp_id}: {e}"))?;
    let arc = Arc::new(webauthn);
    guard.insert(key, arc.clone());
    Ok(arc)
}

#[cfg(test)]
pub(crate) async fn clear_webauthn_cache_for_test() {
    if let Some(cache) = WEBAUTHN_CACHE.get() {
        cache.lock().await.clear();
    }
}

// ---------------------------------------------------------------------------
// ChallengeStore — in-memory pending ceremonies with TTL eviction
// ---------------------------------------------------------------------------

/// What's stashed against a session_id while we await the client's
/// `finish` call. Resident-key login uses `DiscoverableAuthentication`
/// (no allowCredentials list); password-less attempted-id login (not
/// implemented in v1) would use `PasskeyAuthentication`. Both variants
/// are carried so the enum is closed and future migrations are local
/// to this module.
#[derive(Debug)]
pub enum ChallengeState {
    Register {
        state: PasskeyRegistration,
        /// Device that initiated the enrollment. The handler trusts the
        /// envelope's `AuthedDevice` here — the client doesn't get to
        /// pick which device the new credential binds to.
        device_id: String,
        /// RP ID resolved at start-time so finish doesn't have to
        /// re-derive it from headers. Locks in the rp_id stored on the
        /// passkey row for profile-switch drift detection.
        rp_id: String,
    },
    Login(DiscoverableAuthentication),
    /// Non-discoverable login (allowCredentials populated). Not used by
    /// v1 handlers but plumbed for completeness so future "step-up
    /// passkey for sudo" work doesn't need a second store.
    #[allow(dead_code)]
    PasskeyLogin(PasskeyAuthentication),
}

#[derive(Debug)]
struct ChallengeEntry {
    state: ChallengeState,
    /// Monotonic deadline computed at insert time (`now + ttl`). Storing
    /// the absolute deadline rather than the start + ttl spares us a
    /// per-tick recomputation in `evict_expired`.
    expires_at: Instant,
}

/// In-memory map of opaque `session_id -> ChallengeState` with TTL
/// eviction.
///
/// Cheap to clone (Arc internally). Handlers grab a clone from
/// `AppState`. Single-use semantics are enforced by [`Self::take`]: the
/// entry is removed on read so a replay attempt finds an empty slot and
/// fails closed.
#[derive(Clone)]
pub struct ChallengeStore {
    inner: Arc<Mutex<HashMap<String, ChallengeEntry>>>,
}

impl Default for ChallengeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ChallengeStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Insert a new pending ceremony with the given TTL. Returns the
    /// opaque session_id (32 random bytes, base64url-encoded — same
    /// alphabet the device tokens use, so the audit-log filter wouldn't
    /// have to special-case a second format).
    pub async fn insert(&self, state: ChallengeState, ttl: Duration) -> String {
        let session_id = generate_session_id();
        let entry = ChallengeEntry {
            state,
            expires_at: Instant::now() + ttl,
        };
        let mut map = self.inner.lock().await;
        map.insert(session_id.clone(), entry);
        session_id
    }

    /// Atomically take an entry by session_id. Returns `None` if the
    /// session is unknown or expired — the caller is responsible for
    /// mapping `None` to a 400 response.
    ///
    /// The function intentionally does NOT distinguish "unknown" from
    /// "expired": both are user-facing failures with the same remedy
    /// ("start the ceremony again"). Leaking the distinction would let
    /// a scanner probe for valid session_ids to time their attacks
    /// against the TTL window.
    pub async fn take(&self, session_id: &str) -> Option<ChallengeState> {
        let mut map = self.inner.lock().await;
        let entry = map.remove(session_id)?;
        if Instant::now() >= entry.expires_at {
            return None;
        }
        Some(entry.state)
    }

    /// Drop every expired entry. Called by a background tokio task on a
    /// 60-second cadence; also safe to call on demand.
    pub async fn evict_expired(&self) {
        let now = Instant::now();
        let mut map = self.inner.lock().await;
        map.retain(|_, entry| entry.expires_at > now);
    }

    /// Spawn a background tokio task that calls [`Self::evict_expired`]
    /// every `tick`. Returns the task handle so the caller can hold on
    /// to it (drop = abort). The default cadence (60 s) is conservative
    /// vs the typical TTL (300 s) — a single tick is enough to keep the
    /// map under O(in-flight) without piling up.
    pub fn spawn_reaper(&self, tick: Duration) -> tokio::task::JoinHandle<()> {
        let store = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick);
            // Skip the first immediate tick; we just ran the constructor.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                store.evict_expired().await;
            }
        })
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

fn generate_session_id() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG failure generating passkey session id");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// LocalLAN rp_origin derivation
// ---------------------------------------------------------------------------

/// Build the `rp_origin` URL to pair with a resolved RP ID under
/// LocalLAN.
///
/// `AuthPolicy::rp_id_for_origin` collapses every loopback host
/// (`127.0.0.1`, `::1`, `localhost`) to RP ID `"localhost"`. webauthn-rs
/// requires `rp_origin.host()` to match the RP ID exactly (or be a
/// subdomain, if `allow_subdomains` is on — we don't enable it). So a
/// request that arrives on `https://127.0.0.1:3100` must still produce
/// `https://localhost:3100` as the `rp_origin`, or the ceremony fails
/// at verification time with `OriginOverlap`-style errors that are a
/// pain to debug.
///
/// This helper centralizes that derivation so handlers can't accidentally
/// pass the raw request URL through.
pub fn local_lan_rp_origin(port: u16) -> anyhow::Result<Url> {
    let s = format!("https://localhost:{port}");
    Url::parse(&s).map_err(|e| anyhow::anyhow!("constructing LocalLAN rp_origin {s} failed: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_profile::{AuthProfile, RpId};

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url")
    }

    // ── ChallengeStore unit tests ──────────────────────────────────────

    #[tokio::test]
    async fn challenge_store_insert_take_roundtrip() {
        let store = ChallengeStore::new();
        // We can't easily fabricate a real PasskeyRegistration without a
        // SoftPasskey rig (webauthn-rs 0.5 doesn't ship one publicly), so
        // we use a PasskeyLogin entry constructed via the only path
        // available: start a discoverable auth on a real Webauthn and
        // wrap the returned state. That's a separate test below — here
        // we only need to assert insert/take symmetry on a synthetic
        // value.
        //
        // Workaround: build a real DiscoverableAuthentication so the
        // store contains a valid variant. We piggyback on
        // `Webauthn::start_discoverable_authentication`.
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();
        let (_chal, state) = w.start_discoverable_authentication().unwrap();

        let session_id = store
            .insert(ChallengeState::Login(state), Duration::from_secs(5))
            .await;
        assert_eq!(store.len().await, 1);
        let taken = store.take(&session_id).await;
        assert!(taken.is_some(), "take must return the inserted state");
        assert_eq!(store.len().await, 0);
    }

    /// Single-use semantics: a second `take` for the same session_id
    /// must return `None`. This is the replay defense we'll lean on in
    /// the handler tests.
    #[tokio::test]
    async fn challenge_store_take_is_single_use() {
        let store = ChallengeStore::new();
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();
        let (_c, state) = w.start_discoverable_authentication().unwrap();
        let id = store
            .insert(ChallengeState::Login(state), Duration::from_secs(5))
            .await;

        assert!(store.take(&id).await.is_some());
        assert!(
            store.take(&id).await.is_none(),
            "second take must be None — single-use"
        );
    }

    /// TTL expiry: a `take` past the deadline must return `None`, AND
    /// the entry should be evicted by the `evict_expired` sweep so the
    /// map doesn't grow without bound.
    #[tokio::test(start_paused = true)]
    async fn challenge_store_take_after_ttl_is_none() {
        let store = ChallengeStore::new();
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();
        let (_c, state) = w.start_discoverable_authentication().unwrap();
        let id = store
            .insert(ChallengeState::Login(state), Duration::from_millis(50))
            .await;
        tokio::time::advance(Duration::from_millis(60)).await;
        assert!(
            store.take(&id).await.is_none(),
            "expired entries must read as None"
        );
    }

    /// `evict_expired` removes expired rows wholesale, keeping the map
    /// bounded against a "start ceremonies and never finish" abuser.
    #[tokio::test(start_paused = true)]
    async fn challenge_store_evict_expired_clears_old_rows() {
        let store = ChallengeStore::new();
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();

        // Insert three short-TTL entries and one long-TTL entry.
        for _ in 0..3 {
            let (_c, state) = w.start_discoverable_authentication().unwrap();
            store
                .insert(ChallengeState::Login(state), Duration::from_millis(50))
                .await;
        }
        let (_c, state) = w.start_discoverable_authentication().unwrap();
        let long = store
            .insert(ChallengeState::Login(state), Duration::from_secs(60))
            .await;

        assert_eq!(store.len().await, 4);
        tokio::time::advance(Duration::from_millis(60)).await;
        store.evict_expired().await;
        assert_eq!(
            store.len().await,
            1,
            "only the long-TTL entry should remain"
        );
        assert!(store.take(&long).await.is_some());
    }

    // ── webauthn_for cache identity ────────────────────────────────────

    /// Same `(rp_id, rp_origin)` → same `Arc<Webauthn>`. Without this
    /// we'd pay the COSE-algorithm-init cost on every request, defeating
    /// the whole point of the cache.
    #[tokio::test]
    async fn webauthn_for_returns_same_arc_for_same_key() {
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let rp_id = RpId("localhost".to_string());
        let origin = url("https://localhost:3100");
        let a = webauthn_for(&rp_id, &origin, &policy).await.unwrap();
        let b = webauthn_for(&rp_id, &origin, &policy).await.unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "cache hit must return same Arc — got two distinct instances"
        );
    }

    /// Different `(rp_id, rp_origin)` → different `Arc<Webauthn>`. The
    /// cache MUST NOT alias across keys, or an Enterprise deployment
    /// sharing a runtime with a LocalLAN test would silently use the
    /// wrong RP ID.
    #[tokio::test]
    async fn webauthn_for_returns_distinct_arcs_for_distinct_keys() {
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let a = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();
        let b = webauthn_for(
            &RpId("wisphive.example.com".to_string()),
            &url("https://wisphive.example.com"),
            &policy,
        )
        .await
        .unwrap();
        assert!(
            !Arc::ptr_eq(&a, &b),
            "distinct RP IDs must produce distinct Webauthn instances"
        );
    }

    // ── local_lan_rp_origin ────────────────────────────────────────────

    #[test]
    fn local_lan_rp_origin_uses_localhost_name() {
        let u = local_lan_rp_origin(3100).unwrap();
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host_str(), Some("localhost"));
        assert_eq!(u.port(), Some(3100));
    }

    // ── challenge_ttl is honored in BOTH ceremonies ─────────────────────

    /// `AuthPolicy::challenge_ttl` MUST be wired into both the register
    /// and the authenticate ceremonies. The wiring point is
    /// `WebauthnBuilder::timeout(ttl)` at instance build time; both
    /// `start_passkey_registration` and `start_discoverable_authentication`
    /// inherit it. This test asserts the assumption holds by reading the
    /// `timeout` field on the public response shape — webauthn-rs serializes
    /// the timeout into the `CreationChallengeResponse.public_key.timeout`
    /// and `RequestChallengeResponse.public_key.timeout` fields so the
    /// browser knows when to abandon the prompt.
    #[tokio::test]
    async fn challenge_ttl_appears_in_both_register_and_login_responses() {
        let mut policy = AuthProfile::LocalLAN.policy();
        // Use a distinctive non-default TTL so we can prove the wiring
        // isn't accidentally picking up `DEFAULT_AUTHENTICATOR_TIMEOUT`
        // (300s — same as our policy default, which would alias).
        policy.challenge_ttl = Duration::from_secs(125);
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();

        let user_id = uuid::Uuid::new_v4();
        let (creation, _state) = w
            .start_passkey_registration(user_id, "user", "User", None)
            .unwrap();
        // The CreationChallengeResponse contains a `public_key` field
        // whose JSON form carries the timeout as milliseconds.
        let json = serde_json::to_value(&creation).unwrap();
        let reg_timeout = json["publicKey"]["timeout"].as_u64();
        assert_eq!(
            reg_timeout,
            Some(125 * 1000),
            "register ceremony must inherit policy.challenge_ttl as ms"
        );

        let (auth, _state) = w.start_discoverable_authentication().unwrap();
        let json = serde_json::to_value(&auth).unwrap();
        let login_timeout = json["publicKey"]["timeout"].as_u64();
        assert_eq!(
            login_timeout,
            Some(125 * 1000),
            "authenticate ceremony must inherit policy.challenge_ttl as ms"
        );
    }

    /// Reaper smoke: spawn the background sweep with a fast cadence and
    /// confirm an expired row gets cleaned up without anyone calling
    /// `evict_expired` directly. We don't `start_paused` here because
    /// the reaper internally uses `tokio::time::interval`, which under
    /// paused time would need explicit `advance` cooperation across
    /// the spawn boundary — a recipe for flaky tests. The real-time
    /// version finishes in ~150ms which is fine for a unit test.
    #[tokio::test]
    async fn challenge_store_reaper_evicts_expired() {
        let store = ChallengeStore::new();
        let policy = AuthProfile::LocalLAN.policy();
        clear_webauthn_cache_for_test().await;
        let w = webauthn_for(
            &RpId("localhost".to_string()),
            &url("https://localhost:3100"),
            &policy,
        )
        .await
        .unwrap();
        let (_c, state) = w.start_discoverable_authentication().unwrap();
        store
            .insert(ChallengeState::Login(state), Duration::from_millis(20))
            .await;
        assert_eq!(store.len().await, 1);

        let handle = store.spawn_reaper(Duration::from_millis(40));
        // Sleep long enough for the entry to expire AND the reaper to
        // tick at least once past the expiry. The reaper's
        // `set_missed_tick_behavior(Delay)` + initial throwaway tick
        // means we wait ~2 ticks worth: 80ms reaper + 20ms TTL +
        // generous slack.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            store.len().await,
            0,
            "reaper should have swept the expired row"
        );
        handle.abort();
    }
}
