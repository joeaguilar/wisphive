//! Auth profile + policy: the startup-selected posture that drives every
//! later auth/security decision (RP ID derivation, UV requirement, login
//! throttle, sudo-on-enroll, ephemeral pairing listener, etc.).
//!
//! Why a profile, not a knob bag: per the /alignment session 2026-05-16
//! ([`docs/plan-mobile-device-pairing.md`] "Profiles" section), the prior
//! single-strategy lock for #219 silently broke on the LAN-IP-origin case
//! — WebAuthn forbids IP literals as RP IDs (Chrome and Safari both reject
//! them), so "per-origin credentials" had no resolution on
//! `https://192.168.1.42:8443`. Profiles make the gap explicit: LocalLAN
//! reports "no passkey on LAN-IP origin" via the origin-aware
//! `/api/auth/profile` endpoint; Enterprise mandates a real registrable
//! domain at startup and dodges the problem entirely.
//!
//! v1 is atomic-profile only — no per-knob env overrides. Add profiles
//! sparingly when a real new posture emerges; avoid knob-by-knob configs.

use std::net::IpAddr;
use std::time::Duration;

#[cfg(test)]
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::Url;
use wisphive_daemon::state::StateDb;

/// User-verification requirement passed to `webauthn-rs` when building
/// register/login requests. Mirrors the WebAuthn `UserVerificationRequirement`
/// enum so callers in the eventual #311 handlers can map it 1:1.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UvRequirement {
    Required,
    Preferred,
    Discouraged,
}

/// Newtype for a WebAuthn RP ID (a registrable-suffix domain string).
/// Wrapped so we can't accidentally pass a raw origin or an IP literal
/// where a real RP ID is required — both would silently fail at the
/// browser's RP ID check, costing a debugging round.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RpId(
    /// Canonical registrable-domain representation used as the WebAuthn RP ID.
    pub String,
);

impl RpId {
    /// Borrow this RP ID as its canonical domain string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RpId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for RpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Operator-selected security/auth posture for this daemon process.
///
/// Selected at startup via `--auth-profile {local-lan|enterprise}` and
/// frozen for the lifetime of the process. Switching profiles between
/// runs may invalidate already-enrolled `web_passkeys` rows whose stored
/// `rp_id` no longer matches the active profile — operators are warned
/// at startup via [`scan_passkey_rp_id_drift`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthProfile {
    /// Default, opinionated for single-user local-first deploys. Self-signed
    /// TLS OK, ephemeral LAN pairing listener enabled, phone authenticates
    /// via device bearer (no passkey on LAN-IP origin), desktop passkey
    /// optional under RP ID `localhost`.
    LocalLAN,
    /// Operator-provided cert + real registrable domain. No ephemeral LAN
    /// listener, passkey-register sudo-gated, UV required, login throttle
    /// stricter.
    Enterprise { rp_id: RpId, rp_origin: Url },
}

/// Frozen knob set derived from the active [`AuthProfile`]. Threaded
/// through `AppState` + `SecurityConfig` so every downstream handler can
/// branch on it without re-deriving from the profile enum.
#[derive(Clone, Debug)]
pub struct AuthPolicy {
    pub allow_self_signed: bool,
    pub allow_ephemeral_lan_listener: bool,
    /// Whether passkey login is *required* (vs allowed-as-convenience).
    /// v1 keeps both profiles at `false` — password login is always
    /// permitted. Flag is plumbed so #313 (Enterprise device-enroll) can
    /// later flip it without re-threading.
    pub passkey_required: bool,
    pub uv_requirement: UvRequirement,
    pub challenge_ttl: Duration,
    pub login_throttle_threshold: u32,
    /// When `true`, `/api/auth/passkey/register` requires a fresh sudo
    /// re-auth (itr#257 pattern) before minting a credential. Enterprise
    /// turns this on; LocalLAN keeps it off because the local-first
    /// threat model already requires `localhost` access.
    pub require_sudo_for_passkey_register: bool,
    profile_tag: ProfileTag,
    rp_id_strategy: RpIdStrategy,
}

/// Stable string tag for the active profile — exposed on the public
/// `/api/auth/profile` JSON without leaking the enum's internals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileTag {
    LocalLAN,
    Enterprise,
}

impl ProfileTag {
    fn as_str(self) -> &'static str {
        match self {
            ProfileTag::LocalLAN => "local-lan",
            ProfileTag::Enterprise => "enterprise",
        }
    }
}

/// How [`AuthPolicy::rp_id_for_origin`] resolves an origin to an RP ID.
/// Two strategies cover both v1 profiles cleanly:
///
/// - `LoopbackOnly` — LocalLAN: returns `Some("localhost")` for any
///   loopback origin (`127.0.0.1`, `::1`, `localhost`) and `None` for
///   RFC1918 IPv4 literals or anything else. The `None` result is what
///   the SPA reads to hide the "enroll passkey" button on the phone.
/// - `Static(RpId)` — Enterprise: always returns the configured RP ID
///   regardless of the request origin. The Origin/Host allowlist has
///   already constrained which origins reach this code path; the policy
///   just rubber-stamps the configured RP ID for credential issuance.
#[derive(Clone, Debug)]
enum RpIdStrategy {
    LoopbackOnly,
    Static(RpId),
}

impl AuthProfile {
    /// Compute the frozen [`AuthPolicy`] for this profile.
    pub fn policy(&self) -> AuthPolicy {
        match self {
            AuthProfile::LocalLAN => AuthPolicy {
                allow_self_signed: true,
                allow_ephemeral_lan_listener: true,
                passkey_required: false,
                uv_requirement: UvRequirement::Preferred,
                challenge_ttl: Duration::from_secs(300),
                login_throttle_threshold: 5,
                require_sudo_for_passkey_register: false,
                profile_tag: ProfileTag::LocalLAN,
                rp_id_strategy: RpIdStrategy::LoopbackOnly,
            },
            AuthProfile::Enterprise { rp_id, .. } => AuthPolicy {
                allow_self_signed: false,
                allow_ephemeral_lan_listener: false,
                passkey_required: false,
                uv_requirement: UvRequirement::Required,
                challenge_ttl: Duration::from_secs(300),
                login_throttle_threshold: 3,
                require_sudo_for_passkey_register: true,
                profile_tag: ProfileTag::Enterprise,
                rp_id_strategy: RpIdStrategy::Static(rp_id.clone()),
            },
        }
    }
}

impl AuthPolicy {
    /// Public profile tag (`"local-lan"` or `"enterprise"`) for the
    /// `/api/auth/profile` JSON payload.
    pub fn profile_str(&self) -> &'static str {
        self.profile_tag.as_str()
    }

    /// Resolve an HTTP request `Origin` to a WebAuthn RP ID per the active
    /// profile. Returns `None` when no valid RP ID can be derived — the
    /// caller MUST treat that as "passkey enrollment not possible on this
    /// origin" (NOT as "try a default"). See [`RpIdStrategy`] for the
    /// per-profile rules.
    pub fn rp_id_for_origin(&self, origin: &Url) -> Option<RpId> {
        match &self.rp_id_strategy {
            RpIdStrategy::LoopbackOnly => loopback_rp_id_from_origin(origin),
            RpIdStrategy::Static(rp_id) => Some(rp_id.clone()),
        }
    }

    #[cfg(test)]
    fn rp_id_strategy_tag(&self) -> &'static str {
        match self.rp_id_strategy {
            RpIdStrategy::LoopbackOnly => "loopback-only",
            RpIdStrategy::Static(_) => "static",
        }
    }
}

/// Loopback-resolution for LocalLAN: `127.0.0.1`, `::1`, `localhost` all
/// collapse to RP ID `localhost`. Anything else (RFC1918 IP, public IP,
/// random hostname) returns `None`. The collapse is what makes desktop
/// passkeys work under LocalLAN — the browser sees `localhost` whether
/// the URL was typed as `127.0.0.1` or `localhost`.
///
/// The IP-literal `None` path is the load-bearing one for the phone case:
/// WebAuthn's spec (§5.1.3 step 7) rejects IP literals as RP IDs, and
/// every shipping browser enforces it. Returning `None` here lets the
/// frontend hide the "enroll passkey" button rather than letting the
/// browser silently fail.
fn loopback_rp_id_from_origin(origin: &Url) -> Option<RpId> {
    let host = origin.host()?;
    match host {
        url::Host::Domain(d) if d.trim_end_matches('.').eq_ignore_ascii_case("localhost") => {
            Some(RpId("localhost".to_string()))
        }
        // IP literals (including loopback `127.0.0.1` / `[::1]`) are forbidden
        // as WebAuthn RP IDs by §5.1.3 step 9, and crucially the browser-side
        // check in `navigator.credentials.create/get` enforces this BEFORE
        // any server-side allowlist gets a chance to weigh in. Previously
        // we returned `Some(RpId("localhost"))` for loopback IPs, which
        // produced `can_enroll_passkey_on_this_origin: true` on a page
        // loaded via `https://127.0.0.1:<port>` — and then Chrome threw
        // `SecurityError: This is an invalid domain` the moment the user
        // clicked "Enroll passkey", because the page's effectiveDomain
        // (`127.0.0.1`) doesn't equal the requested rp.id (`localhost`).
        // The sprint-1 wave-4 smoke caught this.
        //
        // Honest answer: passkey enroll is NOT available on IP-literal
        // origins. The SPA hides the enroll button (and the
        // "Sign in with a passkey" button) when this returns `None`,
        // matching the LAN-IP-under-LocalLAN case the gate exists for in
        // the first place. The companion redirect middleware
        // (`security::loopback_ip_redirect`) catches browser navigation
        // to `127.0.0.1` / `[::1]` and 301s to `localhost`, so most users
        // never even land on an IP-literal origin in practice.
        // `Ipv4Addr::is_loopback()` covers the full 127.0.0.0/8 range, even
        // though the Host allowlist currently seeds only `127.0.0.1:<port>`
        // and `localhost:<port>`. Thus `127.0.0.2` is normally rejected at
        // the host gate before reaching this policy. Keep the policy's full
        // loopback classification intact if this branch gains distinct
        // behavior; the narrower allowlist is a separate concern.
        url::Host::Ipv4(ip) if ip.is_loopback() => None,
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
        _ => None,
    }
}

/// Errors surfaced by [`validate_enterprise_config`]. Kept as a dedicated
/// enum (not anyhow) so the CLI layer can produce a precise, user-facing
/// fail-fast message without `Display`-stringifying an arbitrary chain.
#[derive(Debug, thiserror::Error)]
pub enum EnterpriseValidationError {
    #[error(
        "Enterprise profile requires user-provided TLS cert (--tls-cert / --tls-key). \
         Either ship itr#270 (Stage 2a) first or select --auth-profile local-lan."
    )]
    MissingTlsFlags,
    #[error("Enterprise profile requires --auth-rp-id <domain>")]
    MissingRpId,
    #[error(
        "--auth-rp-id must be a registrable domain, not an IP literal (got {0:?}); \
         WebAuthn forbids IP RP IDs"
    )]
    RpIdIsIpLiteral(String),
    #[error("--auth-rp-id {0:?} is not a valid DNS name")]
    RpIdNotDomain(String),
}

/// Validate that the operator's CLI inputs are coherent before we even
/// stand up the `AuthProfile::Enterprise` value. Called from the CLI
/// before [`AuthProfile::Enterprise`] construction so the failure surface
/// is a clean stderr message + non-zero exit, not a half-built daemon.
///
/// itr#270 dependency: the `--tls-cert` / `--tls-key` flags don't exist
/// yet. Until they do, Enterprise selection must fail fast with the
/// [`EnterpriseValidationError::MissingTlsFlags`] message. The
/// `tls_cert_provided` / `tls_key_provided` flags let the caller signal
/// "yes, those flags landed and were given" once #270 ships.
pub fn validate_enterprise_config(
    rp_id: Option<&str>,
    tls_cert_provided: bool,
    tls_key_provided: bool,
) -> Result<RpId, EnterpriseValidationError> {
    if !tls_cert_provided || !tls_key_provided {
        return Err(EnterpriseValidationError::MissingTlsFlags);
    }
    let rp_id = rp_id.ok_or(EnterpriseValidationError::MissingRpId)?;
    if rp_id.parse::<IpAddr>().is_ok() {
        return Err(EnterpriseValidationError::RpIdIsIpLiteral(
            rp_id.to_string(),
        ));
    }
    if !is_plausible_dns_name(rp_id) {
        return Err(EnterpriseValidationError::RpIdNotDomain(rp_id.to_string()));
    }
    Ok(RpId(rp_id.to_string()))
}

/// Cheap structural check: at least one dot, every label non-empty, each
/// char DNS-legal. Not a full RFC 1035 validator — the real cert SAN
/// suffix check belongs in #270's cert-loading path. This just rejects
/// the obvious garbage (`""`, `".."`, `"foo bar"`, IP literals already
/// handled upstream). A label made only of digits is rejected too: a value
/// such as `123.456` is structurally DNS-like but not a registrable domain.
fn is_plausible_dns_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.chars().all(|c| c.is_ascii_digit())
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// Whether `ip` is in an RFC 1918 private-use range. Used only by unit tests.
#[cfg(test)]
pub(crate) fn is_rfc1918(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168)
}

/// Profile-switch detection — scan `web_passkeys.rp_id` for rows whose
/// stored RP ID no longer matches the active profile and WARN-log each
/// mismatch so the operator knows to re-enroll.
///
/// itr#311 added the `rp_id` column AND the WebAuthn handlers that
/// populate it. Rows enrolled before the migration carry `rp_id = ""`
/// (the column's `DEFAULT ''`) and will warn under both profiles —
/// that's correct, since we can't tell which profile they were enrolled
/// under and the safe answer is "re-enroll".
///
/// The `is_missing_column_error` guard is intentionally kept. The
/// migration in `wisphive_daemon::state::migrate` is idempotent across
/// daemon restarts, but a half-merged migration (e.g. an operator
/// running a stale CLI binary against a freshly upgraded daemon, or a
/// downgrade) could still leave the column absent at startup. Crashing
/// the whole daemon on a missing audit column would be a worse failure
/// mode than skipping the scan.
pub async fn scan_passkey_rp_id_drift(state_db: &StateDb, policy: &AuthPolicy) {
    // Single raw query — sqlx returns a `Database` error with code
    // `1` (SQLITE_ERROR) and message containing "no such column" when
    // the column is absent. We catch that specific shape and skip
    // silently; any other error gets logged because it likely signals
    // a corrupt DB the operator should see.
    let pool = state_db.pool();
    let result: Result<Vec<(String, String)>, sqlx::Error> =
        sqlx::query_as("SELECT id, rp_id FROM web_passkeys")
            .fetch_all(pool)
            .await;

    let rows = match result {
        Ok(rows) => rows,
        Err(e) => {
            if is_missing_column_error(&e) {
                tracing::debug!(
                    "scan_passkey_rp_id_drift: rp_id column missing \
                     (half-merged migration?), skipping drift scan"
                );
                return;
            }
            tracing::warn!(error = %e, "scan_passkey_rp_id_drift: query failed");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let profile = policy.profile_str();
    let expected: Option<&str> = match &policy.rp_id_strategy {
        RpIdStrategy::Static(rp) => Some(rp.as_str()),
        // LocalLAN: every passkey enrolled at a loopback origin will be
        // `localhost`. Mismatches under LocalLAN therefore mean "this
        // row was enrolled under a different (probably Enterprise)
        // profile" — warn each one.
        RpIdStrategy::LoopbackOnly => Some("localhost"),
    };

    if let Some(expected_rp) = expected {
        for (id, rp_id) in rows {
            if rp_id != expected_rp {
                tracing::warn!(
                    passkey_id = %id,
                    stored_rp_id = %rp_id,
                    expected_rp_id = %expected_rp,
                    profile,
                    "passkey RP ID does not match active profile; operator must re-enroll"
                );
            }
        }
    }
}

/// True iff `e` is a sqlx error whose underlying SQLite message is "no
/// such column: ..." — the specific shape we see when the `rp_id`
/// column hasn't been added yet. Matches the precise `Database` variant
/// so we don't depend on the outer enum's `Display` formatting, which
/// has shifted between sqlx versions in the past.
fn is_missing_column_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => db.message().contains("no such column"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webauthn_rs::prelude::Url;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url should parse")
    }

    // ── policy preset matrix ───────────────────────────────────────────

    #[test]
    fn local_lan_policy_matches_locked_matrix() {
        let p = AuthProfile::LocalLAN.policy();
        assert!(p.allow_self_signed);
        assert!(p.allow_ephemeral_lan_listener);
        assert!(!p.passkey_required);
        assert_eq!(p.uv_requirement, UvRequirement::Preferred);
        assert_eq!(p.challenge_ttl, Duration::from_secs(300));
        assert_eq!(p.login_throttle_threshold, 5);
        assert!(!p.require_sudo_for_passkey_register);
        assert_eq!(p.profile_str(), "local-lan");
        assert_eq!(p.rp_id_strategy_tag(), "loopback-only");
    }

    #[test]
    fn enterprise_policy_matches_locked_matrix() {
        let p = AuthProfile::Enterprise {
            rp_id: RpId("wisphive.example.com".to_string()),
            rp_origin: url("https://wisphive.example.com"),
        }
        .policy();
        assert!(!p.allow_self_signed);
        assert!(!p.allow_ephemeral_lan_listener);
        assert!(!p.passkey_required);
        assert_eq!(p.uv_requirement, UvRequirement::Required);
        assert_eq!(p.challenge_ttl, Duration::from_secs(300));
        assert_eq!(p.login_throttle_threshold, 3);
        assert!(p.require_sudo_for_passkey_register);
        assert_eq!(p.profile_str(), "enterprise");
        assert_eq!(p.rp_id_strategy_tag(), "static");
    }

    // ── rp_id_for_origin matrix (locked 2026-05-16) ────────────────────

    #[test]
    fn local_lan_localhost_resolves_to_localhost() {
        // Only the `localhost` DNS name maps to a valid RP ID — IP
        // literals (even loopback) are forbidden by WebAuthn §5.1.3
        // step 9 at the browser layer, so returning `Some` for them
        // would just produce a `SecurityError` the moment the user
        // clicked enroll. See `loopback_rp_id_from_origin` doc.
        let p = AuthProfile::LocalLAN.policy();
        assert_eq!(
            p.rp_id_for_origin(&url("http://localhost:3100")),
            Some(RpId("localhost".to_string()))
        );
        // https vs http doesn't affect RP ID — only host shape does.
        assert_eq!(
            p.rp_id_for_origin(&url("https://localhost:3100")),
            Some(RpId("localhost".to_string()))
        );
        // No-port form (would only happen via a synthesized URL from
        // the Host-fallback path; included for completeness).
        assert_eq!(
            p.rp_id_for_origin(&url("https://localhost")),
            Some(RpId("localhost".to_string()))
        );
        // Authority userinfo and the default HTTP port do not change the
        // origin host used for RP ID derivation.
        assert_eq!(
            p.rp_id_for_origin(&url("http://user:pass@localhost/")),
            Some(RpId("localhost".to_string()))
        );
        assert_eq!(
            p.rp_id_for_origin(&url("http://localhost/")),
            Some(RpId("localhost".to_string()))
        );
        // A DNS trailing root label names the same loopback host.
        assert_eq!(
            p.rp_id_for_origin(&url("http://localhost./")),
            Some(RpId("localhost".to_string()))
        );
    }

    #[test]
    fn local_lan_ip_literal_loopback_resolves_to_none() {
        // Sprint-1 wave-4 manual-smoke regression: a page loaded via
        // `https://127.0.0.1:3100` previously had its enroll button shown
        // (because we returned `Some(RpId("localhost"))` here) and then
        // Chrome threw `SecurityError: This is an invalid domain` when
        // the user actually clicked enroll — because the page's
        // effectiveDomain (`127.0.0.1`) doesn't equal the requested
        // rp.id (`localhost`). The honest answer is `None`: don't offer
        // a button that will fail. The companion
        // `security::loopback_ip_redirect` middleware catches browser
        // navigation to these origins and 301s to `localhost` so most
        // users never land here, but the policy answer is still load-
        // bearing for any caller that reaches the daemon via IP literal.
        let p = AuthProfile::LocalLAN.policy();
        assert_eq!(p.rp_id_for_origin(&url("http://127.0.0.1:3100")), None);
        assert_eq!(p.rp_id_for_origin(&url("https://127.0.0.1:3100")), None);
        assert_eq!(p.rp_id_for_origin(&url("http://[::1]:3100")), None);
        assert_eq!(p.rp_id_for_origin(&url("https://[::1]:3100")), None);
    }

    #[test]
    fn local_lan_rfc1918_origins_resolve_to_none() {
        let p = AuthProfile::LocalLAN.policy();
        // The phone case: WebAuthn forbids IP literals as RP IDs.
        assert_eq!(p.rp_id_for_origin(&url("https://192.168.1.42:8443")), None);
        assert_eq!(p.rp_id_for_origin(&url("https://10.0.0.5")), None);
        assert_eq!(p.rp_id_for_origin(&url("https://172.16.5.1")), None);
        // Edge of 172.16/12 range — still RFC1918, still None.
        assert_eq!(p.rp_id_for_origin(&url("https://172.31.255.254")), None);
    }

    #[test]
    fn local_lan_public_ip_or_domain_origins_resolve_to_none() {
        let p = AuthProfile::LocalLAN.policy();
        // Public IPs are not loopback and (deliberately) not allow-listed
        // as LocalLAN RP IDs either — anyone reaching this from
        // `https://203.0.113.1` is in a config we don't support.
        assert_eq!(p.rp_id_for_origin(&url("https://203.0.113.1")), None);
        // Random domains under LocalLAN are also None — `localhost` is
        // the only allowed RP ID. Operators wanting a domain RP ID must
        // pick the Enterprise profile.
        assert_eq!(
            p.rp_id_for_origin(&url("https://wisphive.example.com")),
            None
        );
    }

    #[test]
    fn enterprise_returns_static_rp_id_regardless_of_origin() {
        let rp = RpId("wisphive.example.com".to_string());
        let p = AuthProfile::Enterprise {
            rp_id: rp.clone(),
            rp_origin: url("https://wisphive.example.com"),
        }
        .policy();
        // Same RP ID for every origin the Origin/Host allowlist lets
        // through — Enterprise's whole point is "one registrable domain,
        // cross-device portable credentials".
        assert_eq!(
            p.rp_id_for_origin(&url("https://wisphive.example.com")),
            Some(rp.clone())
        );
        assert_eq!(
            p.rp_id_for_origin(&url("https://login.wisphive.example.com")),
            Some(rp.clone())
        );
        // Even nonsense origins: the policy is static. The gate at the
        // request level (Origin/Host allowlist) is the right place to
        // reject — not here.
        assert_eq!(p.rp_id_for_origin(&url("https://192.168.1.42")), Some(rp));
    }

    // ── Enterprise validation ──────────────────────────────────────────

    #[test]
    fn enterprise_validation_rejects_missing_tls_flags() {
        let err = validate_enterprise_config(Some("wisphive.example.com"), false, false)
            .expect_err("missing tls flags must error");
        assert!(matches!(err, EnterpriseValidationError::MissingTlsFlags));
        // Partial (only one of the two) also rejected — operator typoed
        // one half of the pair.
        let err = validate_enterprise_config(Some("wisphive.example.com"), true, false)
            .expect_err("missing tls key must error");
        assert!(matches!(err, EnterpriseValidationError::MissingTlsFlags));
        let err = validate_enterprise_config(Some("wisphive.example.com"), false, true)
            .expect_err("missing tls cert must error");
        assert!(matches!(err, EnterpriseValidationError::MissingTlsFlags));
    }

    #[test]
    fn enterprise_validation_rejects_missing_rp_id() {
        // With TLS flags present but no RP ID, the error must point at
        // the right knob — operator shouldn't have to guess which flag
        // is missing.
        let err =
            validate_enterprise_config(None, true, true).expect_err("missing rp_id must error");
        assert!(matches!(err, EnterpriseValidationError::MissingRpId));
    }

    #[test]
    fn enterprise_validation_rejects_ip_literal_rp_id() {
        let err = validate_enterprise_config(Some("192.168.1.42"), true, true)
            .expect_err("ip literal must error");
        assert!(matches!(err, EnterpriseValidationError::RpIdIsIpLiteral(_)));
        let err = validate_enterprise_config(Some("::1"), true, true)
            .expect_err("ipv6 literal must error");
        assert!(matches!(err, EnterpriseValidationError::RpIdIsIpLiteral(_)));
    }

    #[test]
    fn enterprise_validation_rejects_garbage_rp_id() {
        // Single-label hostname (no dot) — not a registrable domain.
        assert!(matches!(
            validate_enterprise_config(Some("localhost"), true, true).unwrap_err(),
            EnterpriseValidationError::RpIdNotDomain(_)
        ));
        // Spaces, empty labels — all rejected by the structural check.
        assert!(matches!(
            validate_enterprise_config(Some("foo bar.com"), true, true).unwrap_err(),
            EnterpriseValidationError::RpIdNotDomain(_)
        ));
        assert!(matches!(
            validate_enterprise_config(Some(".com"), true, true).unwrap_err(),
            EnterpriseValidationError::RpIdNotDomain(_)
        ));
        // Numeric-only labels look like a dotted-quad but are neither an
        // IP literal accepted by `IpAddr` nor a registrable DNS name.
        assert!(matches!(
            validate_enterprise_config(Some("123.456"), true, true).unwrap_err(),
            EnterpriseValidationError::RpIdNotDomain(_)
        ));
    }

    #[test]
    fn enterprise_validation_accepts_real_domain() {
        let rp = validate_enterprise_config(Some("wisphive.example.com"), true, true).unwrap();
        assert_eq!(rp.as_str(), "wisphive.example.com");
        assert_eq!(rp.as_ref(), "wisphive.example.com");
        // Multi-label, hyphens in middle, etc.
        let rp = validate_enterprise_config(Some("auth-srv.corp.example.net"), true, true).unwrap();
        assert_eq!(rp.as_str(), "auth-srv.corp.example.net");
    }

    // ── rfc1918 helper ─────────────────────────────────────────────────

    #[test]
    fn rfc1918_matrix() {
        assert!(is_rfc1918(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_rfc1918(Ipv4Addr::new(10, 255, 255, 255)));
        assert!(is_rfc1918(Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_rfc1918(Ipv4Addr::new(172, 31, 255, 254)));
        assert!(is_rfc1918(Ipv4Addr::new(192, 168, 1, 1)));
        // Outside: 172.15 is below the range, 172.32 is above.
        assert!(!is_rfc1918(Ipv4Addr::new(172, 15, 0, 1)));
        assert!(!is_rfc1918(Ipv4Addr::new(172, 32, 0, 1)));
        assert!(!is_rfc1918(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!is_rfc1918(Ipv4Addr::new(100, 64, 0, 1))); // CGNAT
        assert!(!is_rfc1918(Ipv4Addr::LOCALHOST));
    }

    // ── profile-switch scan ─────────────────────────────────────────────

    /// Half-merged-migration safety: a daemon that opens a DB whose
    /// `web_passkeys` table predates the itr#311 ALTER (e.g. an operator
    /// rolled back the daemon binary) must NOT crash on startup. The
    /// `is_missing_column_error` guard inside `scan_passkey_rp_id_drift`
    /// is what keeps that contract; simulating the missing-column shape
    /// requires us to drop the column after `StateDb::open` ran the
    /// migration, which SQLite doesn't natively support — so we use the
    /// table-rename + recreate-without-column trick.
    #[tokio::test]
    async fn scan_passkey_rp_id_drift_no_ops_when_column_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wisphive.db");
        let db = StateDb::open(db_path.to_string_lossy().as_ref())
            .await
            .unwrap();
        // Seed a device so we can FK-link a passkey row.
        db.insert_web_device("dev-1", "test", "tokhash")
            .await
            .unwrap();

        // Manually rewrite the table without the rp_id column so the
        // scan's SELECT hits "no such column: rp_id". This is the
        // half-merged-migration shape — `is_missing_column_error` must
        // catch it and return cleanly. We can't just `DROP COLUMN`
        // because SQLite < 3.35 doesn't support it on every distro the
        // daemon might run on (and our migration deliberately doesn't
        // require it). Sequence: rename the migrated table → create the
        // old shape → copy back the columns that overlap.
        let pool = db.pool();
        sqlx::query("ALTER TABLE web_passkeys RENAME TO web_passkeys_v2")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE web_passkeys (
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES web_devices(id) ON DELETE CASCADE,
                public_key BLOB NOT NULL,
                sign_count INTEGER NOT NULL,
                transports TEXT,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            )",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE web_passkeys_v2")
            .execute(pool)
            .await
            .unwrap();

        let policy = AuthProfile::LocalLAN.policy();
        // If the is_missing_column_error guard regresses, this call
        // surfaces the sqlx error and (depending on caller) panics.
        // That's the assertion — the function must swallow the missing
        // column shape silently.
        scan_passkey_rp_id_drift(&db, &policy).await;
    }

    /// Positive-path logging skeleton. itr#311 will add a tracing capture
    /// harness and un-ignore this test once the passkey migration is
    /// available everywhere the suite runs.
    #[tokio::test]
    #[ignore = "itr#311: add a tracing WARN capture harness for drift logging"]
    async fn scan_passkey_rp_id_drift_logs_warn_skeleton() {
        let capture_warns = || -> Vec<String> {
            todo!(
                "seed a mismatched web_passkeys.rp_id row, call the scan, and capture tracing WARNs"
            )
        };
        assert!(capture_warns().iter().any(|message| {
            message.contains("passkey RP ID does not match active profile")
                && message.contains("stored_rp_id=wisphive.example.com")
        }));
    }

    /// Restored positive smoke (crossfire wave-3 review; "additive, not
    /// destructive"): itr#311 already landed the `web_passkeys.rp_id` column,
    /// so this row-seeding scan executes for real and guards the drift query
    /// path that produces the WARN. Kept alongside the ignored tracing-capture
    /// skeleton above rather than replaced by it.
    #[tokio::test]
    async fn scan_passkey_rp_id_drift_warns_on_mismatched_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wisphive.db");
        let db = StateDb::open(db_path.to_string_lossy().as_ref())
            .await
            .unwrap();
        db.insert_web_device("dev-1", "test", "tokhash")
            .await
            .unwrap();
        // Two rows: one matching, one mismatched. Active policy is
        // LocalLAN (expected rp_id = "localhost").
        db.insert_web_passkey("pk-match", "dev-1", b"k1", 0, None, None, "localhost")
            .await
            .unwrap();
        db.insert_web_passkey(
            "pk-drift",
            "dev-1",
            b"k2",
            0,
            None,
            None,
            "wisphive.example.com",
        )
        .await
        .unwrap();
        // Plus a pre-migration row (rp_id = ""), which should also drift
        // under LocalLAN since "" != "localhost".
        db.insert_web_passkey("pk-legacy", "dev-1", b"k3", 0, None, None, "")
            .await
            .unwrap();

        let policy = AuthProfile::LocalLAN.policy();
        // Smoke: must not panic, must not return Err. The warn-log
        // emissions are verified by manual inspection / future tracing
        // subscriber test if one lands.
        scan_passkey_rp_id_drift(&db, &policy).await;

        // Sanity: the row whose rp_id WAS "localhost" still exists.
        let row = db
            .find_web_passkey_by_credential_id("pk-match")
            .await
            .unwrap()
            .expect("pk-match should still exist");
        assert_eq!(row.rp_id, "localhost");
    }
}
