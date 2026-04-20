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
use wisphive_daemon::state::StateDb;
use wisphive_web::auth;
use wisphive_web::tls;

/// `~/.wisphive` — mirrors `hooks::wisphive_home` so we don't drag that
/// private helper out of its module.
fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

fn db_path() -> PathBuf {
    wisphive_home().join("wisphive.db")
}

async fn open_db() -> Result<StateDb> {
    let path = db_path();
    let s = path.to_string_lossy();
    StateDb::open(&s)
        .await
        .with_context(|| format!("opening state db at {s}"))
}

/// Double-prompt for a password with confirmation. Returns `None` if the two
/// entries disagree or the entered password is empty.
fn prompt_password_twice() -> Result<Option<String>> {
    let first = rpassword::prompt_password("New web password: ")?;
    if first.is_empty() {
        eprintln!("empty password — aborted");
        return Ok(None);
    }
    let second = rpassword::prompt_password("Confirm password: ")?;
    if first != second {
        eprintln!("passwords did not match — aborted");
        return Ok(None);
    }
    Ok(Some(first))
}

/// Ask for typed confirmation. Returns `true` only if the operator types
/// `y` / `yes` (case-insensitive).
fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [y/N]: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

/// `wisphive web set-password`
///
/// Double-prompts for a password (via `rpassword` so it doesn't echo),
/// hashes with Argon2id, and upserts into `web_password`. This does NOT
/// touch existing device tokens — already-logged-in browsers keep working
/// until their tokens are revoked or the operator runs `reset-password`.
pub async fn set_password() -> Result<()> {
    let Some(password) = prompt_password_twice()? else {
        return Ok(());
    };
    let db = open_db().await?;
    persist_password(&db, &password).await?;
    eprintln!("Web password updated.");
    Ok(())
}

/// Hash-and-store implementation extracted from [`set_password`] so tests
/// can drive it with a temp-dir `StateDb` (no TTY, no global `$HOME`).
async fn persist_password(db: &StateDb, password: &str) -> Result<()> {
    let phc = auth::hash_password(password).context("hashing password")?;
    db.set_web_password(&phc)
        .await
        .map_err(|e| anyhow::anyhow!("failed to store password hash: {e}"))?;
    Ok(())
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
    if !confirm("Proceed?")? {
        eprintln!("aborted");
        return Ok(());
    }
    let db = open_db().await?;
    db.reset_web_password()
        .await
        .map_err(|e| anyhow::anyhow!("failed to reset web password: {e}"))?;
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
    let devices = db
        .list_web_devices()
        .await
        .map_err(|e| anyhow::anyhow!("failed to list devices: {e}"))?;
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
/// Idempotent — the underlying `UPDATE ... WHERE revoked_at IS NULL` simply
/// does nothing on a second call. We still succeed so scripted revokes stay
/// simple.
pub async fn devices_revoke(id: String) -> Result<()> {
    let db = open_db().await?;
    db.revoke_web_device(&id)
        .await
        .map_err(|e| anyhow::anyhow!("failed to revoke device {id}: {e}"))?;
    eprintln!("Device {id} revoked (or already was).");
    Ok(())
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
            eprintln!(
                "No TLS certificate at {}/web.cert.pem yet.\n\
                 Start the web server once (`wisphive daemon start --web` or `wisphive web serve`) \
                 and the cert will be minted on first run.",
                home.display()
            );
            std::process::exit(1);
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
        let db = StateDb::open(path.to_str().unwrap()).await.unwrap();
        (dir, db)
    }

    /// `persist_password` must store a PHC string that round-trips through
    /// `auth::verify_password` — otherwise a freshly-set password silently
    /// fails every future login.
    #[tokio::test]
    async fn persist_password_round_trips_through_verify() {
        let (_dir, db) = tmp_db().await;
        persist_password(&db, "hunter2-correct-horse")
            .await
            .unwrap();
        let phc = db.get_web_password_hash().await.unwrap().unwrap();
        assert!(auth::verify_password("hunter2-correct-horse", &phc));
        assert!(!auth::verify_password("wrong-password", &phc));
    }

    /// `persist_password` upserts, not inserts: a second call must replace
    /// the first. Without the ON CONFLICT behavior, `set-password` after
    /// an initial setup would silently fail.
    #[tokio::test]
    async fn persist_password_upserts() {
        let (_dir, db) = tmp_db().await;
        persist_password(&db, "first").await.unwrap();
        persist_password(&db, "second").await.unwrap();
        let phc = db.get_web_password_hash().await.unwrap().unwrap();
        assert!(!auth::verify_password("first", &phc));
        assert!(auth::verify_password("second", &phc));
    }
}
