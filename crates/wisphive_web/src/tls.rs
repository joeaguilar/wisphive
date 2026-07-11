//! Self-signed TLS certificate management for the local web UI.
//!
//! Wisphive's web UI is meant to be reachable from the user's LAN (phone on
//! the same Wi-Fi, etc.), so it needs HTTPS. We don't want to drag the user
//! through the joy of running a real CA, so we mint a self-signed ECDSA cert
//! at startup, persist it under `~/.wisphive/`, and surface its SHA-256
//! fingerprint so the user (or pinning code) can verify it out-of-band.
//!
//! Files written:
//! - `<home>/web.cert.pem`        — PEM-encoded cert chain (mode 0600)
//! - `<home>/web.key.pem`         — PEM-encoded PKCS#8 private key (mode 0600)
//! - `<home>/web.cert.meta.json`  — sidecar with `{created_at, sans}` so we
//!   can decide when to regenerate without parsing X.509.
//! - `<home>/web.cert.lock`       — empty lockfile that holds an `flock(2)`
//!   exclusive lock for the duration of `ensure_cert`, so concurrent daemon
//!   starts (or `wisphive web` invocations) can't clobber each other's keys.
//!
//! This module is Unix-only (relies on `flock(2)`, `OpenOptionsExt::mode`,
//! and `PermissionsExt`). The `wisphive_web` crate currently only targets
//! macOS and Linux; if Windows support lands later this whole file needs a
//! `LockFileEx` / `MoveFileEx` rewrite.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rand::TryRngCore;
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration as TimeDuration, OffsetDateTime};
use tracing::warn;

const CERT_FILENAME: &str = "web.cert.pem";
const KEY_FILENAME: &str = "web.key.pem";
const META_FILENAME: &str = "web.cert.meta.json";
const LOCK_FILENAME: &str = "web.cert.lock";
const COMMON_NAME: &str = "Wisphive Web";
/// Regenerate certs older than this (seconds). 90 days.
const MAX_CERT_AGE_SECS: u64 = 90 * 24 * 60 * 60;
/// Cap NotAfter at 397 days from issuance. CA/Browser Forum baseline + iOS
/// Safari + recent Chrome reject longer-lived certs even when self-signed
/// (`ERR_CERT_VALIDITY_TOO_LONG`); without this the phone-on-LAN flow breaks.
/// Renewal is driven by the 90-day rotation policy, so this cap only matters
/// as the absolute upper bound.
const CERT_VALIDITY_DAYS: i64 = 397;
/// Backdate NotBefore by 24h. An hour was the obvious "small skew" choice,
/// but iPhones that haven't synced NTP in a while can be off by days; a
/// 24h pad costs nothing (cert is still 396+d valid) and keeps clock-skewed
/// phones from rejecting the cert with NotYetValid.
const CERT_NOT_BEFORE_BACKDATE_HOURS: i64 = 24;

/// Result of `ensure_cert`: PEM bytes the TLS server needs plus a stable
/// fingerprint suitable for showing in the startup banner.
pub struct EnsureCertResult {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    /// SHA-256 of the DER-encoded certificate, formatted as
    /// `AB:CD:EF:...` (uppercase hex, colon-separated).
    pub fingerprint_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CertMeta {
    /// Unix timestamp (seconds) when the cert was minted.
    created_at: u64,
    /// Sorted list of SANs that were baked into this cert.
    sans: Vec<String>,
}

/// Make sure a usable TLS cert exists at the conventional location, minting a
/// fresh one when files are missing, expired (>90d), or the SAN set has
/// drifted from what we'd generate today (e.g. user changed bind host).
pub fn ensure_cert(home_dir: &Path, bind_host: IpAddr) -> Result<EnsureCertResult> {
    fs::create_dir_all(home_dir)
        .with_context(|| format!("creating home dir {}", home_dir.display()))?;

    // Serialize the whole load-or-generate dance against any other process
    // touching the same home dir. Without this two `wisphive daemon start`
    // calls (or daemon + `wisphive web`) can race and one will hand TLS a
    // cert/key pair from different generations.
    //
    // The binding is named `_cert_lock` (not `_`) deliberately: `let _ = ...`
    // would drop the guard *immediately*, releasing the lock before we do
    // any work. `FileLock` is `#[must_use]` to make that mistake noisy.
    let _cert_lock = FileLock::acquire_exclusive(&home_dir.join(LOCK_FILENAME))?;

    let cert_path = home_dir.join(CERT_FILENAME);
    let key_path = home_dir.join(KEY_FILENAME);
    let meta_path = home_dir.join(META_FILENAME);

    let desired_sans = compute_sans(bind_host);

    if let Some(existing) = try_load_existing(&cert_path, &key_path, &meta_path, &desired_sans)? {
        return Ok(existing);
    }

    generate_and_persist(&cert_path, &key_path, &meta_path, &desired_sans)
}

/// RAII handle around an exclusive `flock(2)` on a lockfile. The kernel
/// releases the lock when the underlying fd closes; we close on drop.
///
/// `flock(2)` semantics caveats worth knowing:
/// - Lock is per *open file description*. Two `acquire_exclusive` calls
///   on the same path each open their own fd, so they contend correctly
///   across threads and processes on a local filesystem.
/// - On NFS, BSD-style flock is silently advisory or no-op on some clients.
///   Wisphive's home dir is `~/.wisphive` so this isn't a concern in
///   practice; if you change the cert path to live on a network share,
///   revisit this.
/// - The lock is on the kernel object, not the directory entry; if the
///   lockfile is `unlink`ed mid-run a later acquirer will create a fresh
///   inode and lock that, defeating serialization. Do not delete the
///   lockfile while wisphive is running.
///
/// itr#234: `acquire_exclusive` also takes an in-process `Mutex<()>` before
/// ever touching the file/flock layer. `flock`'s "per open file description"
/// guarantee (see above) only holds as long as every acquirer opens its own
/// fd; a future refactor that pools or caches fds could put two callers on
/// the *same* fd, at which point flock's per-fd semantics no longer
/// serialize them. The in-process mutex is defense-in-depth against exactly
/// that: two callers in this process always serialize on it regardless of
/// what happens below at the fd/flock layer. It does nothing for
/// cross-process serialization — that's still entirely on `flock`.
#[must_use = "FileLock releases the cert lock when dropped — bind it to a named variable that lives for the whole critical section"]
struct FileLock {
    _file: fs::File,
    // Dropped after `_file` (Rust drops struct fields in declaration order),
    // so release order mirrors acquisition order (mutex first, flock
    // second) in reverse — LIFO, like any other nested-lock discipline.
    _guard: MutexGuard<'static, ()>,
}

/// itr#234: in-process defense-in-depth mutex, taken *before* the OS-level
/// flock in both `FileLock::acquire_exclusive` (production) and
/// `FileLock::acquire_exclusive_on` (test-only). Module-level (rather than
/// function-local) so a test can prove the *actual* static used by
/// production code serializes concurrent acquisitions, not a lookalike.
/// Poison is recovered rather than propagated — see the doc comment on
/// `FileLock` for why.
static IN_PROCESS_LOCK: Mutex<()> = Mutex::new(());

impl FileLock {
    fn acquire_exclusive(path: &Path) -> Result<Self> {
        // In-process defense-in-depth layer, taken before the OS-level
        // flock — see the itr#234 note on the struct doc comment above.
        let guard = IN_PROCESS_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        // We never write to the lockfile — `flock` is on the open fd, not
        // the contents — but we do need `create(true)` for first-run, and
        // `create(true)` requires `write(true)` to succeed on most Unix
        // filesystems. We *don't* truncate, so concurrent openers can't
        // race on length.
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("opening lockfile {}", path.display()))?;
        flock_exclusive(&file, &path.display().to_string())?;
        Ok(Self {
            _file: file,
            _guard: guard,
        })
    }

    /// itr#234 test-only entry point: identical to `acquire_exclusive`
    /// except it takes an already-open `File` instead of a path, so a test
    /// can simulate the hazard this ticket defends against — a caller that
    /// hands `acquire_exclusive` a *cached/shared* fd rather than opening
    /// its own. `File::try_clone` (used by the test) `dup(2)`s the fd,
    /// producing a distinct fd number that nonetheless shares the same
    /// *open file description* as the original — exactly the scenario
    /// where `flock`'s per-fd contention guarantee stops helping (a second
    /// `flock(LOCK_EX)` on a dup'd fd from the same owner succeeds
    /// immediately instead of blocking), leaving the in-process `Mutex` as
    /// the only thing still serializing the two logical acquisitions.
    #[cfg(test)]
    fn acquire_exclusive_on(file: fs::File) -> Result<Self> {
        let guard = IN_PROCESS_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        flock_exclusive(&file, "<shared test fd>")?;
        Ok(Self {
            _file: file,
            _guard: guard,
        })
    }
}

/// Block until `file`'s fd holds an exclusive `flock(2)`. Retries on EINTR
/// so a stray signal (debugger, SIGCHLD) can't bounce the caller out before
/// it locks. `path_for_err` is used only to label the error context.
fn flock_exclusive(file: &fs::File, path_for_err: &str) -> Result<()> {
    let fd = file.as_raw_fd();
    loop {
        // SAFETY: `fd` comes from a `File` the caller owns and outlives the
        // call.
        let r = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if r == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(err).with_context(|| format!("flock(LOCK_EX) on {path_for_err}"));
    }
}

fn try_load_existing(
    cert_path: &Path,
    key_path: &Path,
    meta_path: &Path,
    desired_sans: &[String],
) -> Result<Option<EnsureCertResult>> {
    // Distinguish "file is genuinely missing" (Ok(None) → regenerate) from
    // "we couldn't read it for some other reason" (propagate as Err). The
    // old code lumped both into Ok(None), which under the new flock means
    // an EACCES/EIO/ENFILE silently nukes a cert the user might be
    // debugging — a real correctness regression flagged in review.
    let meta_raw = match fs::read_to_string(meta_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading cert meta {}", meta_path.display()));
        }
    };
    // A corrupt sidecar is suspicious. Earlier this branch warn-and-regen'd
    // — review feedback noted that's inconsistent with the IO-error policy
    // above, which propagates: if EACCES/EIO is a real bug worth surfacing,
    // so is "JSON I just wrote is now garbage". Both indicate something is
    // wrong with the home dir (manual edit, partial write from a pre-fsync
    // codepath, disk corruption) and silently nuking the cert hides that.
    // Propagate as Err; the daemon's outer layer can decide what to do.
    let meta: CertMeta = serde_json::from_str(&meta_raw)
        .with_context(|| format!("parsing cert meta {}", meta_path.display()))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(meta.created_at) > MAX_CERT_AGE_SECS {
        return Ok(None);
    }
    // If created_at is far in the future, the wall clock has been wound
    // back since we minted the cert. Don't trust the age check — force
    // regen so the cert's actual NotAfter (which the browser checks) is
    // back in lockstep with the sidecar.
    if meta.created_at > now.saturating_add(MAX_CERT_AGE_SECS) {
        warn!(
            meta_path = %meta_path.display(),
            created_at = meta.created_at,
            now,
            "cert meta created_at is in the far future; regenerating"
        );
        return Ok(None);
    }

    if meta.sans != desired_sans {
        return Ok(None);
    }

    let cert_pem = match fs::read(cert_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading cert {}", cert_path.display()));
        }
    };
    let key_pem = match fs::read(key_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading key {}", key_path.display()));
        }
    };

    // Parse failure here is "the file exists, we read it, but it isn't a
    // PEM cert" — same data-integrity signal as a corrupt meta sidecar.
    // Propagate consistently rather than silently regen.
    let der = cert_der_from_pem(&cert_pem).with_context(|| {
        format!(
            "cert PEM at {} has no CERTIFICATE block",
            cert_path.display()
        )
    })?;

    // itr#228: cert.pem and key.pem can each parse fine on their own while
    // still being an unusable *pair* — a partial write that only replaced
    // one of the two files, a manual edit, or someone dropping in an
    // unrelated key.pem. Without this check that mismatch isn't caught
    // until a real TLS handshake fails at runtime. Derive the loaded key's
    // public component and compare it against the cert's
    // SubjectPublicKeyInfo; on mismatch, treat it like SAN drift or cert
    // age and fall through to regeneration rather than handing back a pair
    // that can't work together.
    if !key_matches_cert_spki(&key_pem, &der).with_context(|| {
        format!(
            "comparing key {} against cert {} public key",
            key_path.display(),
            cert_path.display()
        )
    })? {
        warn!(
            cert_path = %cert_path.display(),
            key_path = %key_path.display(),
            "loaded private key does not match cert SPKI; regenerating",
        );
        return Ok(None);
    }

    // itr#226: `meta.created_at` above is the *sidecar's* claim about when
    // the cert was minted, and the sidecar is a plain user-writable JSON
    // file — even under a root-owned `~/.wisphive` dir. A tampered or
    // recreated sidecar with a fresh `created_at` would sail through the
    // age check above while pinning an old (possibly compromised) cert
    // forever. Cross-check against the cert's own DER-encoded `NotBefore`
    // — the field a TLS client actually verifies and that can't be edited
    // without re-signing the cert — and regen on that basis too, so a
    // lying sidecar can't override the real cert age.
    let not_before = der_not_before_unix(&der)
        .with_context(|| format!("reading NotBefore from cert {} DER", cert_path.display()))?;
    if (now as i64).saturating_sub(not_before) > MAX_CERT_AGE_SECS as i64 {
        warn!(
            cert_path = %cert_path.display(),
            not_before,
            now,
            "cert DER NotBefore older than policy even though sidecar claims otherwise; regenerating"
        );
        return Ok(None);
    }

    let fingerprint = fingerprint_from_der(&der);

    Ok(Some(EnsureCertResult {
        cert_pem,
        key_pem,
        fingerprint_sha256: fingerprint,
    }))
}

fn generate_and_persist(
    cert_path: &Path,
    key_path: &Path,
    meta_path: &Path,
    desired_sans: &[String],
) -> Result<EnsureCertResult> {
    match generate_and_persist_inner(cert_path, key_path, meta_path, desired_sans) {
        Ok(r) => Ok(r),
        Err(e) => {
            // Each individual write_secret is atomic, but the sequence of
            // three is not: if we crash (or ENOSPC) between writing cert
            // and writing meta, `try_load_existing` on the next run sees
            // cert+key without a matching meta — under the strict error
            // handling above, that's now a regen path, so leftover files
            // would just get overwritten. But cleanup is still the right
            // discipline: don't leave partially-rotated material on disk
            // for some other tool to pick up.
            for p in [meta_path, key_path, cert_path] {
                let _ = fs::remove_file(p);
            }
            Err(e)
        }
    }
}

fn generate_and_persist_inner(
    cert_path: &Path,
    key_path: &Path,
    meta_path: &Path,
    desired_sans: &[String],
) -> Result<EnsureCertResult> {
    #[cfg(test)]
    tests::GENERATE_INVOCATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("generating ECDSA P-256 key pair")?;

    let mut params = CertificateParams::new(desired_sans.to_vec())
        .context("building certificate params from SANs")?;
    params
        .distinguished_name
        .push(DnType::CommonName, COMMON_NAME);
    // rcgen's defaults are NotBefore=1975 / NotAfter=4096 — modern browsers
    // (iOS Safari, Chrome ≥ 85) reject anything over 398 days even on a
    // self-signed cert, so we'd serve an unreachable site. Cap at 397 days
    // and backdate by 24h for clock skew. Day-to-day renewal is still
    // driven by the 90-day rotation in `try_load_existing`.
    let now = OffsetDateTime::now_utc();
    params.not_before = now - TimeDuration::hours(CERT_NOT_BEFORE_BACKDATE_HOURS);
    params.not_after = now + TimeDuration::days(CERT_VALIDITY_DAYS);

    let cert = params
        .self_signed(&key_pair)
        .context("self-signing certificate")?;

    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fingerprint = fingerprint_from_der(cert.der().as_ref());

    // Order matters for crash recovery: write the *key* first so that any
    // partial-success state visible to `try_load_existing` is at worst
    // "key but no cert / meta" — which the NotFound branch turns into a
    // regen. Cert-without-key would let a serving process pick up half a
    // pair if the order were reversed.
    write_secret(key_path, &key_pem)?;
    write_secret(cert_path, &cert_pem)?;

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let meta = CertMeta {
        created_at,
        sans: desired_sans.to_vec(),
    };
    let meta_bytes = serde_json::to_vec_pretty(&meta).context("serializing cert meta")?;
    write_secret(meta_path, &meta_bytes)?;

    Ok(EnsureCertResult {
        cert_pem,
        key_pem,
        fingerprint_sha256: fingerprint,
    })
}

/// Write `bytes` to `path` atomically with `0600` perms.
///
/// Strategy: write to a sibling `<path>.<random-suffix>.tmp`, fchmod via
/// `set_permissions` (in case umask stripped the requested mode), `fsync`
/// so the data is durable, then `rename` over the destination (POSIX
/// guarantees atomic replacement on the same filesystem). Finally `fsync`
/// the containing directory so the rename itself survives a crash.
/// Combined with the `flock` in `ensure_cert`, this gives us: no
/// half-written secrets on disk, and no way for two writers to interleave
/// and produce a cert+key from different generations.
///
/// itr#235: the tmp filename carries a fresh random suffix (via `OsRng`)
/// generated on every call, rather than a fixed `<path>.tmp`. A fixed name
/// is a TOCTOU target: a same-uid attacker could pre-create it as a
/// symlink, and while `create_new`'s `O_EXCL` defeats a symlink-follow on
/// *our* create, a pre-write `remove_file` on that predictable path (to
/// clear a stale tmp) would itself race the attacker's re-create between
/// the unlink and our open. Randomizing the name per write means there is
/// no fixed path left for an attacker to squat on ahead of time.
///
/// On any error after the tmp file is created we best-effort `remove_file`
/// it, so callers don't have to remember to clean up.
fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("path {} has no parent dir", path.display()))?;
    let tmp =
        tmp_path_for(path).with_context(|| format!("computing tmp path for {}", path.display()))?;

    let result = write_secret_inner(&tmp, bytes, dir, path);
    if result.is_err() {
        // Don't leak a half-written tmp if any step after `create_new`
        // failed. Best-effort: if remove also fails, the next call to
        // write_secret on the same final path will clean it up via the
        // `remove_file` above.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_secret_inner(tmp: &Path, bytes: &[u8], dir: &Path, final_path: &Path) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(tmp)
        .with_context(|| format!("creating tmp {}", tmp.display()))?;
    // Belt-and-suspenders fchmod: `mode()` above is honored at create-time
    // but umask can mask bits *out* of it. `File::set_permissions` calls
    // fchmod on the open fd, so any future read of perms sees 0o600. The
    // `O_EXCL` from `create_new` already prevents a symlink-follow attack
    // on the predictable tmp filename; combined with this fchmod the file
    // never appears on disk with mode > 0o600.
    f.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("fchmod tmp {}", tmp.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("writing tmp {}", tmp.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync tmp {}", tmp.display()))?;
    drop(f);

    fs::rename(tmp, final_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), final_path.display()))?;

    // fsync the containing directory so the rename is durable across a
    // power loss. Best-effort: some filesystems don't require this and
    // some platforms (or sandboxes) refuse to open a dir for read. Log
    // on error rather than silently swallow, so a degraded durability
    // story is at least visible in the daemon's structured logs.
    match fs::File::open(dir) {
        Ok(d) => {
            if let Err(e) = d.sync_all() {
                warn!(
                    dir = %dir.display(),
                    error = %e,
                    "directory fsync failed; rename may not survive a crash"
                );
            }
        }
        Err(e) => {
            warn!(
                dir = %dir.display(),
                error = %e,
                "couldn't open dir for fsync; rename may not survive a crash"
            );
        }
    }

    Ok(())
}

/// Sibling tmp path: `web.cert.pem` -> `web.cert.pem.<8-hex-bytes>.tmp`.
///
/// We compute as `parent.join(file_name + "." + suffix + ".tmp")` rather
/// than appending to the full path: the latter trips on a trailing slash
/// (`foo.pem/` → `foo.pem/.tmp`, which is a *child*, not a sibling). None
/// of the current callers pass trailing-slash paths, but a defensive impl
/// catches future refactors.
///
/// itr#235: the suffix is freshly randomized on every call (see
/// `random_tmp_suffix`) rather than a fixed `.tmp`, so the resulting path
/// can't be pre-created by an attacker before we ask to create it.
fn tmp_path_for(path: &Path) -> Result<PathBuf> {
    let dir = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .with_context(|| format!("path {} has no file name", path.display()))?;
    let suffix = random_tmp_suffix().context("generating random tmp suffix")?;
    let mut tmp_name = name.to_owned();
    tmp_name.push(format!(".{suffix}.tmp"));
    Ok(dir.join(tmp_name))
}

/// Generate an 8-byte random suffix (16 lowercase hex chars) from the OS
/// CSPRNG, for use in a randomized tmp filename (itr#235). Sourced from
/// `OsRng` — the same RNG already used elsewhere in this crate for
/// security-sensitive randomness (`auth.rs`, `passkey.rs`) — rather than a
/// PRNG seeded from time/PID, which a same-uid attacker could plausibly
/// predict or narrow down enough to still win the pre-create race.
fn random_tmp_suffix() -> Result<String> {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| anyhow::anyhow!("OsRng fill_bytes failed: {e}"))?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

/// Format a SHA-256 hash of `der` as `AB:CD:...` (uppercase hex, colons).
fn fingerprint_from_der(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let mut out = String::with_capacity(digest.len() * 3);
    for (i, b) in digest.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Parse the `NotBefore` field out of a certificate's DER encoding and
/// return it as Unix seconds.
///
/// This is the itr#226 cross-check: unlike `CertMeta::created_at` (a plain
/// JSON field anyone with write access to the sidecar can edit), `NotBefore`
/// is baked into the signed certificate structure and is exactly what a TLS
/// client validates. Reading it directly from the DER means the age check
/// doesn't have to trust the sidecar at all.
fn der_not_before_unix(cert_der: &[u8]) -> Result<i64> {
    use x509_parser::prelude::FromDer;

    let (_rest, cert) = x509_parser::certificate::X509Certificate::from_der(cert_der)
        .map_err(|e| anyhow::anyhow!("parsing cert DER for NotBefore: {e}"))?;
    Ok(cert.validity().not_before.timestamp())
}

/// Check whether `key_pem` (a PKCS#8 PEM private key) is the key that
/// produced `cert_der`'s SubjectPublicKeyInfo (RFC 5280 §4.1).
///
/// `rcgen::KeyPair::public_key_der()` re-derives the public key from the
/// private key and encodes it as a full SPKI DER structure. `x509-parser`
/// exposes the certificate's parsed SPKI's `raw` field as the equivalent
/// full SPKI DER (algorithm identifier + BIT STRING, not just the raw key
/// bytes) — so a byte-for-byte comparison of the two is exactly "does this
/// key's public half match what this cert was issued for", with no manual
/// ASN.1 field extraction needed on either side.
///
/// Errors (rather than `Ok(false)`) when either input can't be parsed at
/// all — that's a different failure mode than "parses fine but doesn't
/// match" and callers should not conflate the two.
fn key_matches_cert_spki(key_pem: &[u8], cert_der: &[u8]) -> Result<bool> {
    use x509_parser::prelude::FromDer;

    let key_str = std::str::from_utf8(key_pem).context("key PEM is not valid UTF-8")?;
    let key_pair = KeyPair::from_pem(key_str).context("parsing private key PEM")?;
    let key_spki = key_pair.public_key_der();

    let (_rest, cert) = x509_parser::certificate::X509Certificate::from_der(cert_der)
        .map_err(|e| anyhow::anyhow!("parsing cert DER for SPKI comparison: {e}"))?;
    let cert_spki = cert.public_key().raw;

    Ok(key_spki.as_slice() == cert_spki)
}

/// Pull the first CERTIFICATE block out of a PEM blob and return its DER bytes.
fn cert_der_from_pem(pem: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = std::io::BufReader::new(std::io::Cursor::new(pem));
    for item in rustls_pemfile::read_all(&mut cursor).flatten() {
        if let rustls_pemfile::Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

/// Interface name prefixes treated as virtual/ephemeral and therefore
/// excluded from SAN / LAN-URL enumeration (itr#227): Docker bridges
/// (`docker0`, `br-...`), VPN tunnels (`utun*` on macOS, `tun*`/`tap*` on
/// Linux), VirtualBox host-only adapters (`vboxnet*`), and macOS
/// virtualization NICs (`vnic*`). These interfaces come and go independently
/// of the machine's real LAN presence — containers start/stop, VPNs
/// connect/disconnect — so baking their addresses into the cert's SAN set
/// makes `ensure_cert` regenerate on unrelated interface churn, which defeats
/// TOFU cert pinning on phones (itr#283). Matched case-insensitively by
/// prefix so e.g. `docker0`, `Docker Bridge`, `utun7` all match.
const VIRTUAL_IFACE_PREFIXES: &[&str] = &["docker", "br-", "utun", "tun", "tap", "vnic", "vbox"];

/// True if `name` looks like a virtual/ephemeral interface that should be
/// excluded from SAN / LAN-URL enumeration.
fn is_virtual_iface_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIRTUAL_IFACE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// True if `ip` falls in one of the three RFC1918 private ranges
/// (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16).
///
/// Restricting SAN/LAN-URL candidates to RFC1918 also excludes Tailscale's
/// CGNAT range (100.64.0.0/10, RFC 6598) as a side effect, since that block
/// sits outside all three RFC1918 ranges — no separate Tailscale-specific
/// check is needed for this ticket's scope (itr#227).
fn is_rfc1918(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    match o[0] {
        10 => true,
        172 => (16..=31).contains(&o[1]),
        192 => o[1] == 168,
        _ => false,
    }
}

/// Filter a raw `if_addrs` interface list down to the IPv4 addresses we're
/// willing to bake into a cert's SAN set or show in the LAN-URL banner:
/// non-loopback, non-virtual-named (see `is_virtual_iface_name`), and inside
/// an RFC1918 private range.
///
/// Pulled out as a standalone function over `&[if_addrs::Interface]` (rather
/// than inlined at each `if_addrs::get_if_addrs()` call site) so tests can
/// exercise the filtering logic against a synthetic interface list instead
/// of depending on the test runner's real network config (Docker running or
/// not, VPN connected or not) — see the `tests` module below.
fn usable_lan_ipv4_addrs(ifaces: &[if_addrs::Interface]) -> Vec<Ipv4Addr> {
    ifaces
        .iter()
        .filter(|iface| !iface.is_loopback())
        .filter(|iface| !is_virtual_iface_name(&iface.name))
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(v4) if is_rfc1918(v4) => Some(v4),
            _ => None,
        })
        .collect()
}

/// Decide the SAN set for a cert binding to `bind_host`. Sorted for stable
/// equality checks against the persisted meta sidecar.
fn compute_sans(bind_host: IpAddr) -> Vec<String> {
    let mut sans: Vec<String> = Vec::new();
    sans.push("localhost".to_string());
    sans.push("127.0.0.1".to_string());
    if let Some(h) = local_hostname_local() {
        sans.push(h);
    }

    if bind_host == IpAddr::V4(Ipv4Addr::UNSPECIFIED) {
        if let Ok(ifaces) = if_addrs::get_if_addrs() {
            for v4 in usable_lan_ipv4_addrs(&ifaces) {
                sans.push(v4.to_string());
            }
        }
    } else {
        let s = bind_host.to_string();
        if !sans.contains(&s) {
            sans.push(s);
        }
    }

    sans.sort();
    sans.dedup();
    sans
}

/// Best-effort `<hostname>.local` (mDNS-style) name from the OS hostname.
fn local_hostname_local() -> Option<String> {
    let raw = hostname::get().ok()?;
    let s = raw.to_string_lossy().to_string();
    if s.is_empty() {
        return None;
    }
    if s.ends_with(".local") {
        Some(s)
    } else {
        // Strip any existing domain suffix before appending `.local`.
        let base = s.split('.').next().unwrap_or(&s);
        Some(format!("{base}.local"))
    }
}

/// Read the on-disk TLS cert fingerprint without minting a fresh cert.
///
/// Returns `Ok(None)` when the PEM file does not exist — the caller should
/// tell the operator to start the web server once so `ensure_cert` can run.
/// Errors propagate for other IO failures (EACCES, corrupt PEM, etc.) so
/// the operator can see what's wrong rather than getting a silent miss.
///
/// This is deliberately not an `ensure_cert` caller: the fingerprint CLI
/// runs without knowing the bind host, and passing a default (e.g.
/// 127.0.0.1) into `ensure_cert` would trigger a SAN-drift regeneration on
/// a cert originally minted for `0.0.0.0`, silently invalidating any
/// fingerprint the operator already pinned.
pub fn read_cert_fingerprint(home_dir: &Path) -> Result<Option<String>> {
    let cert_path = home_dir.join(CERT_FILENAME);
    let pem = match fs::read(&cert_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading cert {}", cert_path.display()));
        }
    };
    let der = cert_der_from_pem(&pem)
        .with_context(|| format!("no X509 block in {}", cert_path.display()))?;
    Ok(Some(fingerprint_from_der(&der)))
}

/// Produce the list of `https://...:port` URLs we'd happily serve on. Used
/// for the startup banner so the user knows what to type into their phone.
pub fn enumerate_lan_urls(bind_host: IpAddr, port: u16) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    hosts.push("localhost".to_string());
    hosts.push("127.0.0.1".to_string());
    if let Some(h) = local_hostname_local() {
        hosts.push(h);
    }

    if bind_host == IpAddr::V4(Ipv4Addr::UNSPECIFIED) {
        if let Ok(ifaces) = if_addrs::get_if_addrs() {
            for v4 in usable_lan_ipv4_addrs(&ifaces) {
                hosts.push(v4.to_string());
            }
        }
    } else if !bind_host.is_loopback() {
        hosts.push(bind_host.to_string());
    }

    let mut urls: Vec<String> = hosts
        .into_iter()
        .map(|h| format!("https://{h}:{port}"))
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// Test-only counter that `generate_and_persist_inner` bumps on entry.
    /// Used by `concurrent_ensure_cert_serializes` to prove the flock
    /// actually prevented N parallel regenerations (instead of the test
    /// merely passing because OS scheduling happened to serialize the
    /// threads anyway, which would be a false negative for the lock).
    pub(super) static GENERATE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

    /// itr#236: env var carrying the shared home dir a cross-process helper
    /// invocation (`cross_process_helper` below) should race `ensure_cert`
    /// against. Only set by `cross_process_ensure_cert_serializes` when it
    /// re-invokes this very test binary as a subprocess via
    /// `Command::new(current_exe())` — the standard pattern for "spawn a
    /// helper" when a crate has no separate helper binary target. Under a
    /// normal `cargo test` run this var is unset and `cross_process_helper`
    /// is a no-op, so it's harmless for it to run as an ordinary test too.
    const CROSS_PROCESS_DIR_ENV_VAR: &str = "WISPHIVE_TLS_TEST_CROSS_PROCESS_DIR";
    /// itr#236: env var carrying the rendezvous barrier file path. The
    /// helper spins until this file exists before calling `ensure_cert`, so
    /// the parent can release all N helper processes at (approximately) the
    /// same instant instead of them racing in spawn order — mirroring the
    /// `Barrier` used by the in-process `concurrent_ensure_cert_serializes`
    /// test above, but via a filesystem signal since separate OS processes
    /// don't share a `std::sync::Barrier`.
    const CROSS_PROCESS_BARRIER_ENV_VAR: &str = "WISPHIVE_TLS_TEST_CROSS_PROCESS_BARRIER";
    /// itr#236: stdout marker line prefix the helper prints its result
    /// under (`<fingerprint>:<regen-count>`), so the parent can find it amid
    /// normal `cargo test` harness chatter ("running 1 test", "test result:
    /// ok...") that's unavoidable even with `--nocapture`.
    const CROSS_PROCESS_RESULT_MARKER: &str = "WISPHIVE_TLS_TEST_CROSS_PROCESS_RESULT=";

    #[test]
    fn fingerprint_format() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let r = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let fp = &r.fingerprint_sha256;
        // 32 bytes -> 64 hex chars + 31 colons = 95 total.
        assert_eq!(fp.len(), 95, "fingerprint length: {fp}");
        let hex_chars: String = fp.chars().filter(|c| *c != ':').collect();
        assert_eq!(hex_chars.len(), 64);
        assert_eq!(fp.matches(':').count(), 31);
        assert!(
            fp.chars()
                .all(|c| c == ':' || c.is_ascii_digit() || ('A'..='F').contains(&c)),
            "fingerprint must be uppercase hex + colons: {fp}",
        );
    }

    #[test]
    fn san_coverage_loopback_only() {
        let sans = compute_sans(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(sans.contains(&"localhost".to_string()), "sans: {sans:?}");
        assert!(sans.contains(&"127.0.0.1".to_string()), "sans: {sans:?}");
        let has_local = sans.iter().any(|s| s.ends_with(".local"));
        assert!(has_local, "expected a *.local SAN, got {sans:?}");
    }

    #[test]
    fn san_coverage_unspecified() {
        let ifaces = match if_addrs::get_if_addrs() {
            Ok(v) => v,
            Err(_) => return,
        };
        // Guard against the *filtered* set, not just "any non-loopback v4
        // exists" — itr#227 means a machine whose only non-loopback
        // interfaces are virtual (docker0/utun/etc) or outside RFC1918
        // (e.g. a VPN-only CGNAT address) legitimately has nothing to
        // assert here anymore.
        if usable_lan_ipv4_addrs(&ifaces).is_empty() {
            // CI-style sandbox with only loopback/virtual interfaces —
            // nothing to assert.
            return;
        }

        let sans = compute_sans(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert!(sans.contains(&"localhost".to_string()));
        assert!(sans.contains(&"127.0.0.1".to_string()));
        let any_non_loopback_v4 = sans.iter().any(|s| {
            s.parse::<Ipv4Addr>()
                .map(|ip| !ip.is_loopback() && !ip.is_unspecified())
                .unwrap_or(false)
        });
        assert!(
            any_non_loopback_v4,
            "expected a non-loopback IPv4 SAN, got {sans:?}",
        );
    }

    #[test]
    fn reuse_existing_when_valid() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let a = ensure_cert(dir.path(), bind).unwrap();
        let b = ensure_cert(dir.path(), bind).unwrap();
        assert_eq!(
            a.fingerprint_sha256, b.fingerprint_sha256,
            "second call should have reused the cert",
        );
        assert_eq!(a.cert_pem, b.cert_pem);
    }

    #[test]
    fn regen_on_san_drift() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let a = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let b = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::UNSPECIFIED)).unwrap();
        // If this host has any non-loopback IPv4 (or hostname differs), the
        // SAN set will drift and we should regenerate. On a sandbox with
        // only loopback the SAN sets coincide and reuse is correct — accept
        // both outcomes but require regen when drift is actually possible.
        let drift_possible = compute_sans(IpAddr::V4(Ipv4Addr::LOCALHOST))
            != compute_sans(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        if drift_possible {
            assert_ne!(
                a.fingerprint_sha256, b.fingerprint_sha256,
                "SAN drift should have triggered regeneration",
            );
        }
    }

    #[test]
    fn cert_files_have_0600_perms() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let _ = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        for name in [CERT_FILENAME, KEY_FILENAME, META_FILENAME] {
            let p: PathBuf = dir.path().join(name);
            let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name} mode = {mode:o}");
        }
    }

    /// itr#225: validity window must stay under the 398-day cap browsers
    /// enforce. We parse the actual DER (via x509-parser, dev-dep only) so
    /// we're checking what the wire really carries, not just the params we
    /// handed rcgen.
    #[test]
    fn cert_validity_window_under_398_days() {
        use x509_parser::prelude::FromDer;
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let r = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let der = cert_der_from_pem(&r.cert_pem).expect("cert pem should parse");
        let (_rest, cert) = x509_parser::certificate::X509Certificate::from_der(&der)
            .expect("cert DER should parse");
        let validity = cert.validity();
        let window_secs = validity.not_after.timestamp() - validity.not_before.timestamp();
        let window_days = window_secs / 86_400;
        assert!(
            window_days <= 398,
            "validity window {window_days}d exceeds 398-day browser cap",
        );
        assert!(
            window_days >= 396,
            "validity window {window_days}d unexpectedly short — backdate logic regressed?",
        );
        let now = OffsetDateTime::now_utc().unix_timestamp();
        assert!(
            validity.not_before.timestamp() <= now,
            "not_before is in the future; backdate broken",
        );
        assert!(
            validity.not_after.timestamp() > now,
            "not_after is already past; cert is born expired",
        );
    }

    /// itr#224: many threads racing on `ensure_cert` must converge on a
    /// single cert/key pair. Without the flock + atomic write they'd produce
    /// disjoint generations and either disagree on the fingerprint or leave
    /// a cert+key from different generations on disk.
    ///
    /// Hardened against false-passes (review SHOULD-FIX): a `Barrier`
    /// guarantees all N threads are inside the call simultaneously, and
    /// the `GENERATE_INVOCATIONS` counter proves only ONE thread reached
    /// the regeneration path. Without the lock, on a multi-core runner we
    /// would expect 2+ regenerations and either differing fingerprints or
    /// a cert+key mismatch on disk.
    #[test]
    fn concurrent_ensure_cert_serializes() {
        use std::sync::{Arc, Barrier};

        // This test inspects a process-global counter — serialize against
        // any other test that touches `ensure_cert`.
        let _guard = test_lock();

        GENERATE_INVOCATIONS.store(0, Ordering::SeqCst);

        let dir = TempDir::new().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let path = dir.path().to_path_buf();
        const N: usize = 8;
        let barrier = Arc::new(Barrier::new(N));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let p = path.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    // Wait until every thread is at the door before any of
                    // them calls ensure_cert; otherwise on a single-CPU
                    // runner the threads might trivially serialize via the
                    // scheduler and the lock would be untested.
                    b.wait();
                    ensure_cert(&p, bind).unwrap().fingerprint_sha256
                })
            })
            .collect();
        let fingerprints: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let first = &fingerprints[0];
        for f in &fingerprints {
            assert_eq!(
                f, first,
                "concurrent ensure_cert disagreed: {fingerprints:?}"
            );
        }

        let cert_pem = fs::read(path.join(CERT_FILENAME)).unwrap();
        let key_pem = fs::read(path.join(KEY_FILENAME)).unwrap();
        assert!(
            !key_pem.is_empty(),
            "key file is empty after concurrent run"
        );
        let der = cert_der_from_pem(&cert_pem).expect("cert pem should parse");
        let on_disk_fp = fingerprint_from_der(&der);
        assert_eq!(
            &on_disk_fp, first,
            "on-disk cert disagrees with what callers got back",
        );

        // itr#237: fingerprint convergence only proves the N threads agreed
        // on a *label* — it doesn't prove the two files that actually
        // survived the race are a usable pair. A cert from generation A
        // sitting next to a key from generation B could still coincidentally
        // share... no, fingerprints are derived from the cert alone, so this
        // check is the one that actually catches "cert and key are from
        // different generations": cryptographically confirm the on-disk
        // private key's public component matches the on-disk cert's SPKI.
        assert!(
            key_matches_cert_spki(&key_pem, &der).unwrap(),
            "on-disk cert and key are not a matching pair after concurrent ensure_cert",
        );

        let regens = GENERATE_INVOCATIONS.load(Ordering::SeqCst);
        assert_eq!(
            regens, 1,
            "expected exactly one generate_and_persist invocation under the flock; \
             got {regens} — the lock is not actually serializing concurrent callers"
        );
    }

    /// itr#234: proves the in-process `Mutex` added around `flock` actually
    /// does work, rather than being dead defense-in-depth that happens to
    /// pass because `flock` alone would have serialized anyway.
    ///
    /// The scenario this defends against: a future caller that hands
    /// `acquire_exclusive` a *cached/reused* fd instead of opening a fresh
    /// one per call. `flock`'s "per open file description" guarantee (see
    /// the `FileLock` doc comment) means a second `flock(LOCK_EX)` on a
    /// `dup(2)`-derived fd from the *same* open file description succeeds
    /// immediately — it does not block — because the kernel already
    /// considers the calling process the lock owner. So if two logical
    /// "acquire" attempts share an open file description, `flock` alone
    /// would let them both proceed concurrently.
    ///
    /// This test opens one lockfile, hands out `try_clone()`d fds (real
    /// `dup(2)`, same open file description) to N threads, and has each
    /// thread go through `FileLock::acquire_exclusive_on` — the same
    /// `IN_PROCESS_LOCK` static that production `acquire_exclusive` uses.
    /// It tracks how many threads are simultaneously "inside" the guarded
    /// section (via an atomic high-water mark). If the in-process `Mutex`
    /// were missing or wired wrong, `flock` would not save us here (per the
    /// shared-fd behavior above) and we'd observe concurrency > 1.
    #[test]
    fn in_process_mutex_serializes_shared_cached_fd() {
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shared.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        const N: usize = 8;
        let handles: Vec<_> = (0..N)
            .map(|_| {
                // Real dup(2): a distinct fd number sharing the *same* open
                // file description as `file` — the itr#234 hazard scenario.
                let cloned = file.try_clone().unwrap();
                let concurrent = concurrent.clone();
                let max_concurrent = max_concurrent.clone();
                std::thread::spawn(move || {
                    let _lock = FileLock::acquire_exclusive_on(cloned).unwrap();
                    let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_concurrent.fetch_max(now, Ordering::SeqCst);
                    // Hold the section briefly so overlapping acquires (if
                    // the mutex weren't serializing them) have a real
                    // window to be observed concurrently, rather than the
                    // race being too fast to ever manifest.
                    std::thread::sleep(std::time::Duration::from_millis(15));
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            max_concurrent.load(Ordering::SeqCst),
            1,
            "two acquire_exclusive_on calls sharing the same open file \
             description ran inside the guarded section concurrently — the \
             in-process Mutex is not serializing same-fd acquisitions"
        );
    }

    /// itr#236: not a real test on its own — a helper entry point that
    /// `cross_process_ensure_cert_serializes` re-invokes as a *separate OS
    /// process* via `Command::new(current_exe()).arg("cross_process_helper")`.
    /// Under a normal `cargo test` run (no `CROSS_PROCESS_DIR_ENV_VAR` set)
    /// this is a harmless no-op, so it's safe to leave as an ordinary
    /// `#[test]` rather than `#[ignore]`.
    ///
    /// When invoked as a helper: spins on a filesystem barrier so the parent
    /// can release every helper process at roughly the same instant, calls
    /// `ensure_cert` against the shared target dir, and prints the resulting
    /// fingerprint plus this process's own `GENERATE_INVOCATIONS` count
    /// (0 or 1 — each helper is a fresh process with its own copy of that
    /// static) so the parent can sum regenerations across all helpers.
    #[test]
    fn cross_process_helper() {
        let Ok(dir) = std::env::var(CROSS_PROCESS_DIR_ENV_VAR) else {
            return; // not invoked as a cross-process helper — no-op.
        };
        let barrier_path = std::env::var(CROSS_PROCESS_BARRIER_ENV_VAR)
            .expect("barrier env var must be set alongside the dir env var");

        // Spin until the parent releases the starting gun. Bounded so a bug
        // in the parent (or a missing barrier write) can't hang the suite.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !Path::new(&barrier_path).exists() {
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for cross-process start barrier at {barrier_path}");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let result = ensure_cert(Path::new(&dir), bind).expect("helper ensure_cert failed");
        let regens = GENERATE_INVOCATIONS.load(Ordering::SeqCst);
        println!(
            "{CROSS_PROCESS_RESULT_MARKER}{}:{regens}",
            result.fingerprint_sha256
        );
    }

    /// itr#236 (review SHOULD-FIX #6 on `concurrent_ensure_cert_serializes`):
    /// the thread-based test above proves `flock`-per-fd correctness within
    /// ONE process, which is real but doesn't cover the actual production
    /// hazard — two separate `wisphive daemon start` PROCESSES racing to
    /// mint the cert. Threads in one process share the same fd table in
    /// ways a genuine cross-process race does not, so a lock bug that only
    /// manifests across process boundaries could hide behind a green
    /// thread-only test.
    ///
    /// This test spawns N real child processes (by re-invoking this very
    /// test binary via `Command::new(current_exe())` — the standard
    /// "spawn a helper" trick when a crate has no separate helper binary
    /// target) that all race `ensure_cert` against the same shared home
    /// dir, and asserts they converge on exactly one fingerprint and
    /// exactly one regeneration across all of them.
    #[test]
    fn cross_process_ensure_cert_serializes() {
        let dir = TempDir::new().unwrap();
        let dir_str = dir
            .path()
            .to_str()
            .expect("tempdir path must be utf8")
            .to_string();
        let barrier_dir = TempDir::new().unwrap();
        let barrier_path = barrier_dir.path().join("start.go");
        let barrier_str = barrier_path.to_str().unwrap().to_string();

        let exe =
            std::env::current_exe().expect("current_exe should resolve to the test binary itself");

        const N: usize = 5;
        let mut children = Vec::with_capacity(N);
        for _ in 0..N {
            let child = std::process::Command::new(&exe)
                .arg("cross_process_helper")
                .arg("--nocapture")
                .env(CROSS_PROCESS_DIR_ENV_VAR, &dir_str)
                .env(CROSS_PROCESS_BARRIER_ENV_VAR, &barrier_str)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("failed to spawn cross-process helper");
            children.push(child);
        }

        // Give every child a moment to reach the barrier spin-loop, then
        // release them all at once so the ensure_cert calls genuinely
        // overlap instead of racing purely in spawn order.
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&barrier_path, b"go").expect("writing start barrier");

        let mut fingerprints = Vec::with_capacity(N);
        let mut total_regens = 0usize;
        for (i, child) in children.into_iter().enumerate() {
            let output = child
                .wait_with_output()
                .expect("failed to wait on cross-process helper");
            assert!(
                output.status.success(),
                "helper process {i} exited with {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            let line = stdout
                .lines()
                .find(|l| l.starts_with(CROSS_PROCESS_RESULT_MARKER))
                .unwrap_or_else(|| {
                    panic!("helper process {i} printed no result marker; stdout:\n{stdout}")
                });
            let payload = &line[CROSS_PROCESS_RESULT_MARKER.len()..];
            // The fingerprint itself is colon-separated hex (`AB:CD:...`),
            // so split on the *last* colon to separate it from the trailing
            // `:<regen-count>` rather than the first.
            let (fp, regens) = payload
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("malformed helper result line: {line}"));
            fingerprints.push(fp.to_string());
            total_regens += regens
                .trim()
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("malformed regen count in: {line}"));
        }

        let first = fingerprints[0].clone();
        for (i, fp) in fingerprints.iter().enumerate() {
            assert_eq!(
                fp, &first,
                "cross-process ensure_cert disagreed: process {i} got {fp}, \
                 expected {first} ({fingerprints:?})"
            );
        }
        assert_eq!(
            total_regens, 1,
            "expected exactly one regeneration across {N} cross-process racers, \
             got {total_regens} — flock is not serializing across process boundaries"
        );

        let cert_pem = fs::read(dir.path().join(CERT_FILENAME)).unwrap();
        let key_pem = fs::read(dir.path().join(KEY_FILENAME)).unwrap();
        let der = cert_der_from_pem(&cert_pem).expect("cert pem should parse");
        let on_disk_fp = fingerprint_from_der(&der);
        assert_eq!(
            on_disk_fp, first,
            "on-disk cert disagrees with what the cross-process racers got back",
        );
        assert!(
            key_matches_cert_spki(&key_pem, &der).unwrap(),
            "on-disk cert and key are not a matching pair after cross-process ensure_cert race",
        );
    }

    /// Process-wide mutex for tests that read/write `GENERATE_INVOCATIONS`.
    /// Cargo runs tests in parallel by default; without this serialization
    /// other ensure_cert tests would race the counter.
    ///
    /// Poison is treated as a hard error (not recovered): a poisoned lock
    /// means a prior test panicked *inside* the critical section, which
    /// almost certainly left the counter in an unknown state. Recovering
    /// blindly would cause the next test to either silently pass (false
    /// negative on the lock test) or fail for the wrong reason.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test_lock poisoned — a prior ensure_cert test panicked inside the critical section; check the first failure")
    }

    /// `read_cert_fingerprint` returns `Ok(None)` on a fresh home dir
    /// (no cert) — the CLI relies on this to give the operator a pointed
    /// "run the server once" message rather than a cryptic IO error.
    #[test]
    fn read_fingerprint_missing_cert_is_none() {
        let dir = TempDir::new().unwrap();
        let r = read_cert_fingerprint(dir.path()).unwrap();
        assert!(r.is_none(), "empty home dir should yield None, got {r:?}");
    }

    /// Once `ensure_cert` has run, `read_cert_fingerprint` must return the
    /// exact same fingerprint — without regenerating, and byte-for-byte.
    /// itr#215: the `wisphive web fingerprint` CLI is pointless if it
    /// disagrees with what the server logs at startup.
    #[test]
    fn read_fingerprint_matches_ensure_cert() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let ensured = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let read = read_cert_fingerprint(dir.path())
            .unwrap()
            .expect("cert should exist after ensure_cert");
        assert_eq!(ensured.fingerprint_sha256, read);
    }

    /// itr#224: no `<file>.tmp` should linger after a successful run; if one
    /// did, a future run might see it and a recovery tool might mistake it
    /// for the real cert.
    ///
    /// itr#235: `tmp_path_for` now mints a fresh random suffix on every
    /// call, so calling it again here would just produce a brand-new path
    /// that (trivially, uselessly) never existed. Scan the directory for
    /// *any* leftover `*.tmp` entry instead — that's the actual invariant
    /// this test is protecting.
    #[test]
    fn no_tmp_files_left_behind() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let _ = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let leftover: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(
            leftover.is_empty(),
            "leftover tmp file(s) after successful write: {leftover:?}",
        );
    }

    /// itr#228: if `web.key.pem` gets swapped for an unrelated (but
    /// well-formed) key — partial write, manual edit, wrong file copied in —
    /// `ensure_cert` must not hand back the mismatched pair. It should
    /// detect the SPKI mismatch and regenerate both files instead of
    /// leaving a cert/key combo that will only fail at TLS-handshake time.
    #[test]
    fn regen_when_key_does_not_match_cert() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);

        let first = ensure_cert(dir.path(), bind).unwrap();

        // Swap in an unrelated key pair — same algorithm, different
        // material — while leaving cert.pem and the meta sidecar as-is.
        let unrelated_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .unwrap()
            .serialize_pem();
        write_secret(&dir.path().join(KEY_FILENAME), unrelated_key.as_bytes()).unwrap();

        let second = ensure_cert(dir.path(), bind).unwrap();

        assert_ne!(
            first.fingerprint_sha256, second.fingerprint_sha256,
            "a mismatched key/cert pair should have triggered regeneration",
        );
        assert_ne!(
            first.key_pem, second.key_pem,
            "regeneration should have produced a fresh key too",
        );

        // The regenerated pair on disk must actually match each other.
        let cert_pem = fs::read(dir.path().join(CERT_FILENAME)).unwrap();
        let key_pem = fs::read(dir.path().join(KEY_FILENAME)).unwrap();
        let der = cert_der_from_pem(&cert_pem).expect("cert pem should parse");
        assert!(
            key_matches_cert_spki(&key_pem, &der).unwrap(),
            "regenerated cert/key pair should match",
        );
    }

    /// Sanity check on the comparison primitive itself, independent of the
    /// full `ensure_cert` flow: a key paired with its own cert matches, an
    /// unrelated key does not.
    #[test]
    fn key_matches_cert_spki_detects_mismatch() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let ensured = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let der = cert_der_from_pem(&ensured.cert_pem).expect("cert pem should parse");

        assert!(
            key_matches_cert_spki(&ensured.key_pem, &der).unwrap(),
            "a cert/key pair minted together should match",
        );

        let unrelated_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .unwrap()
            .serialize_pem();
        assert!(
            !key_matches_cert_spki(unrelated_key.as_bytes(), &der).unwrap(),
            "an unrelated key should not match the cert's SPKI",
        );
    }

    /// `tmp_path_for` should produce a sibling, not a child, even on
    /// pathological trailing-slash inputs.
    #[test]
    fn tmp_path_is_sibling_not_child() {
        let p = Path::new("/tmp/foo/web.cert.pem");
        let tmp = tmp_path_for(p).unwrap();
        assert_eq!(tmp.parent(), p.parent());
        let name = tmp.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with("web.cert.pem.") && name.ends_with(".tmp"),
            "unexpected tmp filename shape: {name}",
        );
    }

    /// itr#235: the tmp filename must carry a random suffix, not the old
    /// fixed `<file>.tmp` — otherwise a same-uid attacker could pre-create
    /// the exact path (e.g. as a symlink) ahead of time. Assert two calls
    /// for the same target path produce different tmp paths, and that the
    /// suffix looks like the 16-hex-char (8-byte) shape `random_tmp_suffix`
    /// produces.
    #[test]
    fn tmp_path_for_is_randomized_per_call() {
        let p = Path::new("/tmp/foo/web.cert.pem");
        let a = tmp_path_for(p).unwrap();
        let b = tmp_path_for(p).unwrap();
        assert_ne!(
            a, b,
            "tmp_path_for should mint a fresh random suffix every call, got {a:?} twice",
        );
        for tmp in [&a, &b] {
            let name = tmp.file_name().unwrap().to_string_lossy().to_string();
            let suffix = name
                .strip_prefix("web.cert.pem.")
                .and_then(|s| s.strip_suffix(".tmp"))
                .unwrap_or_else(|| panic!("unexpected tmp filename shape: {name}"));
            assert_eq!(
                suffix.len(),
                16,
                "expected a 16-hex-char (8-byte) suffix, got {suffix:?} in {name}",
            );
            assert!(
                suffix.chars().all(|c| c.is_ascii_hexdigit()),
                "suffix should be hex, got {suffix:?} in {name}",
            );
        }
    }

    /// itr#226: a tampered/recreated `web.cert.meta.json` sidecar with a
    /// *fresh* `created_at` must not be able to pin a stale cert forever.
    /// We mint a cert whose DER `NotBefore` is older than
    /// `MAX_CERT_AGE_SECS`, drop it on disk next to a sidecar that lies
    /// about `created_at` being recent, and confirm `ensure_cert` still
    /// regenerates — proving the age check reads the DER, not just the
    /// sidecar.
    #[test]
    fn regen_when_der_not_before_older_than_policy_even_if_sidecar_fresh() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let desired_sans = compute_sans(bind);

        // Mint a cert with a DER NotBefore far in the past — older than
        // policy allows — independent of the normal generate_and_persist
        // path (which always backdates by only 24h).
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(desired_sans.clone()).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, COMMON_NAME);
        let old_not_before =
            OffsetDateTime::now_utc() - TimeDuration::seconds(MAX_CERT_AGE_SECS as i64 + 3600);
        params.not_before = old_not_before;
        params.not_after = old_not_before + TimeDuration::days(CERT_VALIDITY_DAYS);
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem().into_bytes();
        let key_pem = key_pair.serialize_pem().into_bytes();

        fs::create_dir_all(dir.path()).unwrap();
        write_secret(&dir.path().join(KEY_FILENAME), &key_pem).unwrap();
        write_secret(&dir.path().join(CERT_FILENAME), &cert_pem).unwrap();

        // The sidecar lies: created_at claims "right now", not the cert's
        // real (old) NotBefore.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let lying_meta = CertMeta {
            created_at: now,
            sans: desired_sans,
        };
        let meta_bytes = serde_json::to_vec_pretty(&lying_meta).unwrap();
        write_secret(&dir.path().join(META_FILENAME), &meta_bytes).unwrap();

        let old_fp = fingerprint_from_der(&cert_der_from_pem(&cert_pem).unwrap());

        GENERATE_INVOCATIONS.store(0, Ordering::SeqCst);
        let result = ensure_cert(dir.path(), bind).unwrap();

        assert_ne!(
            result.fingerprint_sha256, old_fp,
            "stale DER NotBefore should have triggered regeneration even though \
             the sidecar's created_at claimed the cert was fresh",
        );
        assert_eq!(
            GENERATE_INVOCATIONS.load(Ordering::SeqCst),
            1,
            "expected exactly one regeneration triggered by the DER NotBefore check",
        );
    }

    /// MUST-FIX from review: `try_load_existing` used to swallow read errors
    /// other than NotFound, hiding real bugs (EACCES, EIO) behind a silent
    /// regen. Make a cert+key+meta triple unreadable and confirm we get an
    /// `Err`, not a fresh cert.
    #[test]
    fn unreadable_cert_propagates_error() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
        // First run: mints a valid cert.
        let _ = ensure_cert(dir.path(), bind).unwrap();
        // Strip read perms on the meta sidecar (cheapest of the three to
        // make unreadable). On the next ensure_cert call, we should NOT
        // silently regenerate — we should bubble the IO error.
        let meta = dir.path().join(META_FILENAME);
        let mut perms = fs::metadata(&meta).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&meta, perms).unwrap();

        let result = ensure_cert(dir.path(), bind);

        // Restore perms before unwinding so TempDir::drop can clean up.
        let mut restore = fs::metadata(&meta).unwrap().permissions();
        restore.set_mode(0o600);
        let _ = fs::set_permissions(&meta, restore);

        // Skip the assertion when running as root (e.g. some CI containers),
        // where mode 0o000 is still readable. Detect via uid==0.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        assert!(
            result.is_err(),
            "expected error from unreadable meta, got Ok — IO errors are being silently swallowed"
        );
    }

    /// Build a synthetic `if_addrs::Interface` for a given name/IPv4 pair,
    /// for tests that need to exercise interface filtering without
    /// depending on the test runner's real network config (Docker running
    /// or not, VPN connected or not, actual LAN present or not).
    fn fake_iface(name: &str, ip: Ipv4Addr) -> if_addrs::Interface {
        if_addrs::Interface {
            name: name.to_string(),
            addr: if_addrs::IfAddr::V4(if_addrs::Ifv4Addr {
                ip,
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                prefixlen: 24,
                broadcast: None,
            }),
            index: Some(1),
        }
    }

    /// itr#227: interface names matching the known-virtual prefixes must be
    /// rejected regardless of case, and ordinary NIC names must not be.
    #[test]
    fn virtual_iface_name_matching() {
        for name in [
            "docker0",
            "Docker0",
            "br-abc123",
            "utun0",
            "utun7",
            "tun0",
            "tap0",
            "vnic1",
            "vboxnet0",
            "VBoxNet0",
        ] {
            assert!(
                is_virtual_iface_name(name),
                "{name} should be classified as virtual",
            );
        }
        for name in ["en0", "eth0", "wlan0", "Wi-Fi", "Ethernet"] {
            assert!(
                !is_virtual_iface_name(name),
                "{name} should NOT be classified as virtual",
            );
        }
    }

    /// itr#227: RFC1918 classification, including the CGNAT (Tailscale)
    /// exclusion that falls out of it for free.
    #[test]
    fn rfc1918_classification() {
        for ip in [
            Ipv4Addr::new(10, 0, 0, 5),
            Ipv4Addr::new(10, 255, 255, 254),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(172, 31, 255, 254),
            Ipv4Addr::new(192, 168, 1, 42),
        ] {
            assert!(is_rfc1918(ip), "{ip} should be RFC1918 private");
        }
        for ip in [
            Ipv4Addr::new(100, 64, 0, 1),  // Tailscale CGNAT (RFC 6598)
            Ipv4Addr::new(100, 100, 0, 1), // still within 100.64.0.0/10
            Ipv4Addr::new(172, 15, 0, 1),  // just outside 172.16.0.0/12
            Ipv4Addr::new(172, 32, 0, 1),  // just outside 172.16.0.0/12
            Ipv4Addr::new(192, 169, 0, 1), // not 192.168.0.0/16
            Ipv4Addr::new(8, 8, 8, 8),     // public
        ] {
            assert!(!is_rfc1918(ip), "{ip} should NOT be RFC1918 private");
        }
    }

    /// itr#227 core acceptance: Docker interface churn must not perturb the
    /// filtered address set. We build a synthetic "before Docker started"
    /// interface list and a "Docker running" list that adds docker0/br-*/
    /// utun*/vboxnet* entries (including one, docker0, that sits in a
    /// RFC1918-looking 172.16/12 range — proving the name filter, not just
    /// the range filter, is doing real work) and assert the two produce an
    /// identical usable-address set. Since `compute_sans`/`enumerate_lan_urls`
    /// are thin wrappers over `usable_lan_ipv4_addrs`, this is the direct
    /// proof that `ensure_cert`'s SAN set — and therefore its fingerprint —
    /// stays stable across Docker/VPN interface churn.
    #[test]
    fn usable_addrs_stable_across_docker_vpn_churn() {
        let real_nic = fake_iface("en0", Ipv4Addr::new(192, 168, 1, 42));
        let loopback = if_addrs::Interface {
            name: "lo0".to_string(),
            addr: if_addrs::IfAddr::V4(if_addrs::Ifv4Addr {
                ip: Ipv4Addr::LOCALHOST,
                netmask: Ipv4Addr::new(255, 0, 0, 0),
                prefixlen: 8,
                broadcast: None,
            }),
            index: Some(0),
        };

        let before_docker = vec![loopback.clone(), real_nic.clone()];
        let after_docker_and_vpn = vec![
            loopback,
            real_nic,
            fake_iface("docker0", Ipv4Addr::new(172, 17, 0, 1)),
            fake_iface("br-2f9a8c1e3b4d", Ipv4Addr::new(172, 18, 0, 1)),
            fake_iface("utun7", Ipv4Addr::new(100, 101, 102, 103)), // Tailscale-ish CGNAT
            fake_iface("vboxnet0", Ipv4Addr::new(192, 168, 56, 1)),
        ];

        let before = usable_lan_ipv4_addrs(&before_docker);
        let after = usable_lan_ipv4_addrs(&after_docker_and_vpn);

        assert_eq!(
            before,
            vec![Ipv4Addr::new(192, 168, 1, 42)],
            "expected only the real NIC address before Docker/VPN churn",
        );
        assert_eq!(
            before, after,
            "Docker/VPN interface churn must not change the usable address set: \
             before={before:?} after={after:?}",
        );
    }

    /// itr#227: the LAN URL list must omit docker0/utun*/etc even when
    /// those interfaces have plausible-looking (or RFC1918) addresses.
    #[test]
    fn usable_addrs_omit_docker_and_vpn_interfaces() {
        let ifaces = vec![
            fake_iface("en0", Ipv4Addr::new(192, 168, 1, 42)),
            fake_iface("docker0", Ipv4Addr::new(172, 17, 0, 1)),
            fake_iface("utun4", Ipv4Addr::new(100, 64, 0, 5)),
            fake_iface("tap0", Ipv4Addr::new(10, 8, 0, 1)),
            fake_iface("vnic1", Ipv4Addr::new(192, 168, 55, 2)),
        ];

        let usable = usable_lan_ipv4_addrs(&ifaces);

        assert_eq!(
            usable,
            vec![Ipv4Addr::new(192, 168, 1, 42)],
            "docker0/utun4/tap0/vnic1 addresses must all be filtered out, got {usable:?}",
        );
    }
}
