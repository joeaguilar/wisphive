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
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
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
#[must_use = "FileLock releases the cert lock when dropped — bind it to a named variable that lives for the whole critical section"]
struct FileLock {
    _file: fs::File,
}

impl FileLock {
    fn acquire_exclusive(path: &Path) -> Result<Self> {
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
        // Block until we own the exclusive lock. Retry on EINTR so a stray
        // signal (debugger, SIGCHLD) can't bounce us out before we lock.
        let fd = file.as_raw_fd();
        loop {
            // SAFETY: `fd` comes from a `File` we own and outlives the call.
            let r = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if r == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err).with_context(|| format!("flock(LOCK_EX) on {}", path.display()));
        }
        Ok(Self { _file: file })
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
/// Strategy: write to a sibling `<path>.tmp`, fchmod via `set_permissions`
/// (in case umask stripped the requested mode), `fsync` so the data is
/// durable, then `rename` over the destination (POSIX guarantees atomic
/// replacement on the same filesystem). Finally `fsync` the containing
/// directory so the rename itself survives a crash. Combined with the
/// `flock` in `ensure_cert`, this gives us: no half-written secrets on
/// disk, and no way for two writers to interleave and produce a cert+key
/// from different generations.
///
/// On any error after the tmp file is created we best-effort `remove_file`
/// it, so callers don't have to remember to clean up.
fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("path {} has no parent dir", path.display()))?;
    let tmp =
        tmp_path_for(path).with_context(|| format!("computing tmp path for {}", path.display()))?;

    // A previous crash (or a stale tmp from before `flock` existed) could
    // leave the tmp lying around. We hold the directory's flock so nobody
    // else is writing it concurrently — safe to remove and recreate.
    let _ = fs::remove_file(&tmp);

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

/// Sibling tmp path: `web.cert.pem` -> `web.cert.pem.tmp`.
///
/// We compute as `parent.join(file_name + ".tmp")` rather than appending
/// to the full path: the latter trips on a trailing slash (`foo.pem/` →
/// `foo.pem/.tmp`, which is a *child*, not a sibling). None of the
/// current callers pass trailing-slash paths, but a defensive impl
/// catches future refactors.
fn tmp_path_for(path: &Path) -> Result<PathBuf> {
    let dir = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .with_context(|| format!("path {} has no file name", path.display()))?;
    let mut tmp_name = name.to_owned();
    tmp_name.push(".tmp");
    Ok(dir.join(tmp_name))
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
            for iface in ifaces {
                if iface.is_loopback() {
                    continue;
                }
                if let IpAddr::V4(v4) = iface.ip() {
                    sans.push(v4.to_string());
                }
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
            for iface in ifaces {
                if iface.is_loopback() {
                    continue;
                }
                if let IpAddr::V4(v4) = iface.ip() {
                    hosts.push(v4.to_string());
                }
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
        let has_non_loopback_v4 = ifaces
            .iter()
            .any(|i| !i.is_loopback() && matches!(i.ip(), IpAddr::V4(_)));
        if !has_non_loopback_v4 {
            // CI-style sandbox with only loopback — nothing to assert.
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

        let regens = GENERATE_INVOCATIONS.load(Ordering::SeqCst);
        assert_eq!(
            regens, 1,
            "expected exactly one generate_and_persist invocation under the flock; \
             got {regens} — the lock is not actually serializing concurrent callers"
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

    /// itr#224: no `<file>.tmp` should linger after a successful run; if one
    /// did, a future run might see it and a recovery tool might mistake it
    /// for the real cert.
    #[test]
    fn no_tmp_files_left_behind() {
        let _guard = test_lock();
        let dir = TempDir::new().unwrap();
        let _ = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        for name in [CERT_FILENAME, KEY_FILENAME, META_FILENAME] {
            let tmp = tmp_path_for(&dir.path().join(name)).expect("tmp_path_for");
            assert!(
                !tmp.exists(),
                "leftover tmp file {} after successful write",
                tmp.display(),
            );
        }
    }

    /// `tmp_path_for` should produce a sibling, not a child, even on
    /// pathological trailing-slash inputs.
    #[test]
    fn tmp_path_is_sibling_not_child() {
        let p = Path::new("/tmp/foo/web.cert.pem");
        let tmp = tmp_path_for(p).unwrap();
        assert_eq!(tmp, PathBuf::from("/tmp/foo/web.cert.pem.tmp"));
        assert_eq!(tmp.parent(), p.parent());
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
}
