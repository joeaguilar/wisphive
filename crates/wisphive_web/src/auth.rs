//! Authentication primitives for the local web UI.
//!
//! Three pieces:
//! - **Password hashing** using Argon2id (PHC string output).
//! - **Device tokens** — opaque random bearer tokens; only the SHA-256 hash
//!   is persisted. The raw token is shown to the client exactly once.
//! - **Login throttle** — per-IP exponential backoff to slow down brute force.
//!
//! Constant-time comparison is hand-rolled to avoid pulling `subtle` into the
//! dependency tree.
//
// TODO(webauthn): passkey register/verify

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio::time::Instant;

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
/// Returns `false` on length mismatch (without leaking which side was longer)
/// and `false` if the stored hash isn't valid hex.
pub fn verify_device_token(presented_raw: &str, stored_hash_hex: &str) -> bool {
    let computed = sha256_hex(presented_raw.as_bytes());
    constant_time_eq_str(&computed, stored_hash_hex)
}

/// SHA-256 → lowercase hex.
fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        // `format!` per byte is fine; this is not on a hot path and it
        // avoids pulling in the `hex` crate.
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Constant-time byte compare. Returns `false` if lengths differ — but the
/// length check happens up front and we still walk the shorter buffer to
/// keep timing characteristics consistent for equal-length inputs.
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut acc: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

// ---------------------------------------------------------------------------
// Login throttle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct AttemptState {
    failures: u32,
    locked_until: Instant,
}

/// Per-IP login throttle with exponential backoff.
#[derive(Debug, Clone, Default)]
pub struct LoginThrottle {
    inner: Arc<RwLock<HashMap<IpAddr, AttemptState>>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ThrottleDecision {
    pub allowed: bool,
    pub retry_after: Option<Duration>,
}

impl LoginThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check whether `ip` is currently allowed to attempt a login.
    pub async fn check(&self, ip: IpAddr) -> ThrottleDecision {
        let map = self.inner.read().await;
        match map.get(&ip) {
            Some(state) => {
                let now = Instant::now();
                if now < state.locked_until {
                    ThrottleDecision {
                        allowed: false,
                        retry_after: Some(state.locked_until - now),
                    }
                } else {
                    ThrottleDecision {
                        allowed: true,
                        retry_after: None,
                    }
                }
            }
            None => ThrottleDecision {
                allowed: true,
                retry_after: None,
            },
        }
    }

    /// Record a failed attempt. Increments the counter and pushes
    /// `locked_until` forward according to the backoff schedule.
    pub async fn record_failure(&self, ip: IpAddr) {
        let mut map = self.inner.write().await;
        let entry = map.entry(ip).or_insert(AttemptState {
            failures: 0,
            locked_until: Instant::now(),
        });
        entry.failures = entry.failures.saturating_add(1);
        entry.locked_until = Instant::now() + backoff_for(entry.failures);
    }

    /// Record a successful login. Clears the per-IP counter.
    pub async fn record_success(&self, ip: IpAddr) {
        let mut map = self.inner.write().await;
        map.remove(&ip);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::Ipv4Addr;

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
        throttle.record_failure(ip).await;
        let d = throttle.check(ip).await;
        assert!(!d.allowed);
        let ra = d.retry_after.unwrap();
        assert!(
            ra >= Duration::from_millis(200) && ra <= Duration::from_millis(260),
            "N=1 retry_after expected ~250ms, got {ra:?}"
        );
        tokio::time::advance(Duration::from_millis(260)).await;
        assert!(throttle.check(ip).await.allowed);

        // N=2 → ~500ms, N=3 → ~1s
        throttle.record_failure(ip).await; // failures=2
        throttle.record_failure(ip).await; // failures=3
        let d = throttle.check(ip).await;
        assert!(!d.allowed);
        let ra = d.retry_after.unwrap();
        assert!(
            ra >= Duration::from_millis(900) && ra <= Duration::from_millis(1100),
            "N=3 retry_after expected ~1s, got {ra:?}"
        );
        tokio::time::advance(Duration::from_millis(1100)).await;

        // Push to N=8+ — schedule caps at 30s.
        for _ in 0..6 {
            throttle.record_failure(ip).await;
        }
        let d = throttle.check(ip).await;
        assert!(!d.allowed);
        let ra = d.retry_after.unwrap();
        assert!(
            ra >= Duration::from_millis(29_500) && ra <= Duration::from_secs(30),
            "N>=8 retry_after expected ~30s cap, got {ra:?}"
        );

        // One more failure — should still be capped.
        throttle.record_failure(ip).await;
        let d = throttle.check(ip).await;
        let ra = d.retry_after.unwrap();
        assert!(ra <= Duration::from_secs(30), "should never exceed 30s cap");
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_success_clears() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        throttle.record_failure(ip).await;
        throttle.record_failure(ip).await;
        throttle.record_failure(ip).await;
        assert!(!throttle.check(ip).await.allowed);

        throttle.record_success(ip).await;
        let d = throttle.check(ip).await;
        assert!(d.allowed, "success should clear lockout immediately");
        assert!(d.retry_after.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_per_ip_independent() {
        let throttle = LoginThrottle::new();
        let bad: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let good: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();

        for _ in 0..5 {
            throttle.record_failure(bad).await;
        }
        assert!(!throttle.check(bad).await.allowed);
        let d = throttle.check(good).await;
        assert!(d.allowed, "neighbour IP should be unaffected");
        assert!(d.retry_after.is_none());
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
}
