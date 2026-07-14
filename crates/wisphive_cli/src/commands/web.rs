//! CLI handlers for `wisphive web {set-password, reset-password, devices,
//! fingerprint}`. These talk to the SQLite state DB directly (not via the
//! daemon socket) because they need to work whether or not the daemon is
//! running — after all, you're setting the password *before* you can log in.
//!
//! SQLite's WAL mode + connection pooling means a running daemon sharing
//! `~/.wisphive/wisphive.db` sees the writes on its next query; no IPC
//! needed.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use wisphive_daemon::state::{StateDb, WebAuthError};
use wisphive_web::auth;
use wisphive_web::tls;
use zeroize::Zeroize;

/// `~/.wisphive` — mirrors `hooks::wisphive_home` so we don't drag that
/// private helper out of its module.
fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

fn db_path() -> PathBuf {
    wisphive_home().join("wisphive.db")
}

/// Open the state DB in client mode. CLI admin commands share the DB with
/// a possibly-running daemon, so we must skip the daemon-only startup
/// hook that flips `running` PTY rows to `orphaned` — running it from the
/// CLI would corrupt a live daemon's terminal session state (itr#215
/// review sec#5).
async fn open_db() -> Result<StateDb> {
    let path = db_path();
    let s = path.to_string_lossy();
    StateDb::open_client(&s)
        .await
        .with_context(|| format!("opening state db at {s}"))
}

/// Double-prompt for a password with confirmation. Returns `None` if the two
/// entries disagree or the entered password is empty.
fn prompt_password_twice() -> Result<Option<String>> {
    let mut first = rpassword::prompt_password("New web password: ")?;
    if first.is_empty() {
        eprintln!("empty password — aborted");
        first.zeroize();
        return Ok(None);
    }
    let mut second = match rpassword::prompt_password("Confirm password: ") {
        Ok(password) => password,
        Err(error) => {
            first.zeroize();
            return Err(error.into());
        }
    };
    if first != second {
        eprintln!("passwords did not match — aborted");
        first.zeroize();
        second.zeroize();
        return Ok(None);
    }
    second.zeroize();
    Ok(Some(first))
}

/// Require the operator to type `expected` literally (case-sensitive) to
/// confirm a destructive action. Case-sensitive so a CapsLock user doesn't
/// accidentally match and a muscle-memory `y` doesn't satisfy it either.
fn confirm_typed(expected: &str) -> Result<bool> {
    eprint!("Type {expected} to confirm: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim() == expected)
}

/// `wisphive web set-password`
///
/// Double-prompts for a password (via `rpassword` so it doesn't echo),
/// hashes with Argon2id, and upserts into `web_password`. This does NOT
/// touch existing device tokens — already-logged-in browsers keep working
/// until their tokens are revoked or the operator runs `reset-password`.
pub async fn set_password() -> Result<()> {
    let Some(mut password) = prompt_password_twice()? else {
        return Ok(());
    };
    let db = match open_db().await {
        Ok(db) => db,
        Err(error) => {
            password.zeroize();
            return Err(error);
        }
    };
    persist_password(&db, &mut password).await?;
    eprintln!("Web password updated.");
    Ok(())
}

/// Hash-and-store implementation extracted from [`set_password`] so tests
/// can drive it with a temp-dir `StateDb` (no TTY, no global `$HOME`).
async fn persist_password(db: &StateDb, password: &mut String) -> Result<()> {
    let phc = hash_password_zeroizing(password)?;
    db.set_web_password(&phc)
        .await
        .context("storing password hash in state db")?;
    Ok(())
}

/// Hash a plaintext password, then wipe its caller-owned buffer regardless
/// of whether hashing succeeds.
fn hash_password_zeroizing(password: &mut String) -> Result<String> {
    let result = auth::hash_password(password).context("hashing password");
    password.zeroize();
    result
}

/// `wisphive web reset-password`
///
/// Wipes the password row AND every device token + enrolled passkey.
/// After this, the web UI returns `setup_required` again and the operator
/// has to re-run `set-password`. Destructive — gated behind a typed
/// confirmation so a fat-fingered invocation doesn't log out the LAN.
pub async fn reset_password() -> Result<()> {
    eprintln!(
        "This wipes the web password, ALL trusted devices, and ALL enrolled passkeys.\n\
         The audit log is preserved. Anyone currently logged in will be forced to re-auth."
    );
    // Require a typed literal rather than `y/N`: review sec#2 noted that a
    // single return keystroke (or a buffered newline from a piped stdin)
    // could confirm a destructive wipe by accident. `RESET` is short but
    // impossible to type unintentionally.
    if !confirm_typed("RESET")? {
        eprintln!("aborted");
        return Ok(());
    }
    let db = open_db().await?;
    db.reset_web_password()
        .await
        .context("resetting web password + devices + passkeys")?;
    eprintln!("Web password + devices + passkeys wiped. Run `wisphive web set-password` to seed.");
    Ok(())
}

/// `wisphive web devices list`
///
/// Prints every device row newest-first, active and revoked alike, so the
/// operator can spot stragglers before revoking. Output is tabular text —
/// deliberately not JSON, because this command's audience is a human with a
/// terminal, not a pipeline.
pub async fn devices_list() -> Result<()> {
    let db = open_db().await?;
    let devices = db.list_web_devices().await.context("listing web devices")?;
    if devices.is_empty() {
        eprintln!(
            "No trusted devices. Run `wisphive web set-password` then log in from a browser."
        );
        return Ok(());
    }
    // Column widths sized for typical contents; UUIDv4 is 36 chars, IP max
    // is 15 chars for v4 / 39 for v6. Names come from the browser's "name
    // this device" input so we cap them to 24 chars and truncate with `…`
    // for longer ones — rare, but a pasted User-Agent would otherwise wreck
    // the columns.
    println!(
        "{:<36}  {:<24}  {:<20}  {:<20}  {:<15}  STATUS",
        "ID", "NAME", "CREATED", "LAST SEEN", "LAST IP"
    );
    for d in devices {
        let name = if d.name.chars().count() > 24 {
            let mut t: String = d.name.chars().take(23).collect();
            t.push('…');
            t
        } else {
            d.name.clone()
        };
        let last_seen = d.last_seen_at.as_deref().unwrap_or("-");
        let last_ip = d.last_ip.as_deref().unwrap_or("-");
        let status = match d.revoked_at.as_deref() {
            None => "active".to_string(),
            Some(at) => format!("revoked {at}"),
        };
        println!(
            "{:<36}  {:<24}  {:<20}  {:<20}  {:<15}  {}",
            d.id, name, d.created_at, last_seen, last_ip, status
        );
    }
    Ok(())
}

/// `wisphive web devices revoke <id>`
///
/// Idempotent for existing devices: a second call succeeds, while an unknown
/// ID returns an error so scripts cannot silently accept a typo.
pub async fn devices_revoke(id: String) -> Result<()> {
    let db = open_db().await?;
    revoke_device(&db, &id).await
}

async fn revoke_device(db: &StateDb, id: &str) -> Result<()> {
    match db.revoke_web_device(id).await {
        Ok(()) => {
            eprintln!("Device {id} revoked (or already was).");
            Ok(())
        }
        Err(WebAuthError::NotFound) => Err(anyhow::anyhow!("unknown device id: {id}")),
        Err(error) => Err(error).with_context(|| format!("revoking device {id}")),
    }
}

/// `wisphive web fingerprint`
///
/// Prints the SHA-256 fingerprint of the persisted TLS cert so the operator
/// can pin it out-of-band (read it off the server terminal, type it into
/// the phone's verification prompt on first connect, etc.). Does NOT mint a
/// fresh cert if one is missing — regenerating without a known bind host
/// could silently change the SAN set and invalidate a previously pinned
/// fingerprint.
pub fn fingerprint() -> Result<()> {
    let home = wisphive_home();
    match tls::read_cert_fingerprint(&home)? {
        Some(fp) => {
            println!("{fp}");
            Ok(())
        }
        None => {
            // Return `Err` rather than `std::process::exit(1)` so the
            // caller (`main`'s `?`) handles the exit code uniformly — and
            // so tests can observe the failure without the process dying
            // under them (review eff-NIT).
            Err(anyhow::anyhow!(
                "no TLS certificate at {}/web.cert.pem yet — start the web server once \
                 (`wisphive daemon start --web` or `wisphive web serve`) so the cert is minted",
                home.display()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Open a throwaway `StateDb` under a tmp dir. Mirrors the pattern
    /// `wisphive_daemon::state`'s own tests use.
    async fn tmp_db() -> (TempDir, StateDb) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.db");
        // Tests exercise the client-mode opener so the CLI path is
        // actually tested (not a daemon-mode open that would differ).
        let db = StateDb::open_client(path.to_str().unwrap()).await.unwrap();
        (dir, db)
    }

    /// `persist_password` must store a PHC string that round-trips through
    /// `auth::verify_password` — otherwise a freshly-set password silently
    /// fails every future login.
    #[tokio::test]
    async fn persist_password_round_trips_through_verify() {
        let (_dir, db) = tmp_db().await;
        let mut password = "hunter2-correct-horse".to_string();
        persist_password(&db, &mut password).await.unwrap();
        assert!(password.is_empty());
        let phc = db.get_web_password_hash().await.unwrap().unwrap();
        assert!(auth::verify_password("hunter2-correct-horse", &phc));
        assert!(!auth::verify_password("wrong-password", &phc));
    }

    /// Hashing must not leave the prompt's plaintext in its caller-owned
    /// allocation after Argon2 has consumed it.
    #[test]
    fn hash_password_zeroizes_plaintext_buffer() {
        let mut password = "hunter2-correct-horse".to_string();

        let phc = hash_password_zeroizing(&mut password).unwrap();

        assert!(!phc.is_empty());
        assert!(password.is_empty());
    }

    /// `persist_password` upserts, not inserts: a second call must replace
    /// the first. Without the ON CONFLICT behavior, `set-password` after
    /// an initial setup would silently fail.
    #[tokio::test]
    async fn persist_password_upserts() {
        let (_dir, db) = tmp_db().await;
        let mut first = "first".to_string();
        persist_password(&db, &mut first).await.unwrap();
        let mut second = "second".to_string();
        persist_password(&db, &mut second).await.unwrap();
        let phc = db.get_web_password_hash().await.unwrap().unwrap();
        assert!(!auth::verify_password("first", &phc));
        assert!(auth::verify_password("second", &phc));
    }

    #[tokio::test]
    async fn revoke_unknown_device_returns_a_clear_error() {
        let (_dir, db) = tmp_db().await;

        let error = revoke_device(&db, "unknown-uuid")
            .await
            .expect_err("an unknown device ID must not be accepted as success");

        assert!(
            error
                .to_string()
                .contains("unknown device id: unknown-uuid"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn revoke_known_already_revoked_device_is_idempotent() {
        let (_dir, db) = tmp_db().await;
        db.insert_web_device("known-uuid", "phone", "token-hash")
            .await
            .unwrap();
        db.revoke_web_device("known-uuid").await.unwrap();

        revoke_device(&db, "known-uuid").await.unwrap();

        let device = db
            .list_web_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|device| device.id == "known-uuid")
            .unwrap();
        assert!(device.revoked_at.is_some());
    }

    #[tokio::test]
    async fn revoke_active_device_succeeds_and_revokes_it() {
        let (_dir, db) = tmp_db().await;
        db.insert_web_device("active-uuid", "phone", "token-hash")
            .await
            .unwrap();

        revoke_device(&db, "active-uuid").await.unwrap();

        let device = db
            .list_web_devices()
            .await
            .unwrap()
            .into_iter()
            .find(|device| device.id == "active-uuid")
            .unwrap();
        assert!(device.revoked_at.is_some());
    }
}
