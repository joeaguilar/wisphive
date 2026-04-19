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

use std::fs;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CERT_FILENAME: &str = "web.cert.pem";
const KEY_FILENAME: &str = "web.key.pem";
const META_FILENAME: &str = "web.cert.meta.json";
const COMMON_NAME: &str = "Wisphive Web";
/// Regenerate certs older than this (seconds). 90 days.
const MAX_CERT_AGE_SECS: u64 = 90 * 24 * 60 * 60;

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

    let cert_path = home_dir.join(CERT_FILENAME);
    let key_path = home_dir.join(KEY_FILENAME);
    let meta_path = home_dir.join(META_FILENAME);

    let desired_sans = compute_sans(bind_host);

    if let Some(existing) = try_load_existing(&cert_path, &key_path, &meta_path, &desired_sans)? {
        return Ok(existing);
    }

    generate_and_persist(&cert_path, &key_path, &meta_path, &desired_sans)
}

fn try_load_existing(
    cert_path: &Path,
    key_path: &Path,
    meta_path: &Path,
    desired_sans: &[String],
) -> Result<Option<EnsureCertResult>> {
    if !cert_path.exists() || !key_path.exists() || !meta_path.exists() {
        return Ok(None);
    }
    let meta_raw = match fs::read_to_string(meta_path) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let meta: CertMeta = match serde_json::from_str(&meta_raw) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(meta.created_at) > MAX_CERT_AGE_SECS {
        return Ok(None);
    }

    if meta.sans != desired_sans {
        return Ok(None);
    }

    let cert_pem = match fs::read(cert_path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let key_pem = match fs::read(key_path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    let der = match cert_der_from_pem(&cert_pem) {
        Some(d) => d,
        None => return Ok(None),
    };
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
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("generating ECDSA P-256 key pair")?;

    let mut params = CertificateParams::new(desired_sans.to_vec())
        .context("building certificate params from SANs")?;
    params
        .distinguished_name
        .push(DnType::CommonName, COMMON_NAME);
    // Note: we lean on rcgen's default not_before/not_after window. Our own
    // 90-day rotation policy is enforced via the `created_at` timestamp in
    // the meta sidecar (see `try_load_existing`), which is the actual source
    // of truth for renewals — parsing X.509 dates back from PEM is more
    // dependency than it's worth for a self-signed local cert.

    let cert = params
        .self_signed(&key_pair)
        .context("self-signing certificate")?;

    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fingerprint = fingerprint_from_der(cert.der().as_ref());

    write_secret(cert_path, &cert_pem)?;
    write_secret(key_path, &key_pem)?;

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

/// Write a file with `0600` perms, replacing any existing file at that path.
fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    // Belt-and-suspenders: re-set perms in case umask trumped `mode()`.
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
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
    use tempfile::TempDir;

    #[test]
    fn fingerprint_format() {
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
        let has_non_loopback_v4 = ifaces.iter().any(|i| {
            !i.is_loopback() && matches!(i.ip(), IpAddr::V4(_))
        });
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
        let dir = TempDir::new().unwrap();
        let _ = ensure_cert(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        for name in [CERT_FILENAME, KEY_FILENAME, META_FILENAME] {
            let p: PathBuf = dir.path().join(name);
            let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name} mode = {mode:o}");
        }
    }
}
