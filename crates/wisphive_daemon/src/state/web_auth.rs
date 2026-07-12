use super::StateDb;

/// Typed error surface for the web-auth helpers. Auth callers need to
/// distinguish `NotFound` (→ 401/404) from `Duplicate` (→ 409) from `Db`
/// (→ 500) and from `Revoked` (→ 401 + throttle bump). Using `anyhow` here
/// would collapse those into stringly-typed guesses.
#[derive(Debug, thiserror::Error)]
pub enum WebAuthError {
    /// No row matched the lookup (device id, token hash, passkey id, etc.).
    #[error("web auth target not found")]
    NotFound,
    /// The device exists but has been revoked.
    #[error("web device is revoked")]
    Revoked,
    /// Unique-constraint violation (e.g. duplicate device id or token hash).
    #[error("web auth duplicate")]
    Duplicate,
    /// Underlying database / sqlx error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl WebAuthError {
    /// Classify a sqlx error, promoting UNIQUE constraint failures to
    /// `Duplicate` so callers can map them to 409 without string matching.
    pub(super) fn from_sqlx(err: sqlx::Error) -> Self {
        if let Some(db_err) = err.as_database_error()
            && db_err.message().contains("UNIQUE constraint failed")
        {
            return Self::Duplicate;
        }
        Self::Db(err)
    }
}

pub type WebAuthResult<T> = std::result::Result<T, WebAuthError>;

/// Row shape for `web_devices` fetches that include `revoked_at`.
type WebDeviceFullRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Row shape for `web_devices` lookups that only load active-device fields.
type WebDeviceActiveRow = (String, String, String, Option<String>, Option<String>);

/// Row shape for `web_audit` queries.
type WebAuditRowRaw = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A row from `web_devices`. `revoked_at` is `None` for active devices.
#[derive(Debug, Clone)]
pub struct WebDeviceRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub last_ip: Option<String>,
    pub revoked_at: Option<String>,
}

/// A row from `web_audit`.
#[derive(Debug, Clone)]
pub struct WebAuditRow {
    pub id: i64,
    pub at: String,
    pub event: String,
    pub device_id: Option<String>,
    pub ip: Option<String>,
    pub detail: Option<String>,
}

impl StateDb {
    // ── Web UI auth helpers ───────────────────────────────────────
    //
    // All helpers return `WebAuthResult<T>` (not `anyhow::Result`) so auth
    // callers can distinguish NotFound / Revoked / Duplicate / Db without
    // string-matching on error messages.

    /// Upsert the single-row web password hash.
    pub async fn set_web_password(&self, argon2_hash: &str) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_password (id, argon2_hash, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET argon2_hash = excluded.argon2_hash, updated_at = excluded.updated_at",
        )
        .bind(argon2_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Atomic first-set: returns `true` iff no password existed before this
    /// call. The onboarding endpoint uses this instead of check-then-upsert
    /// so two concurrent first-run set-password requests can't both
    /// "succeed" — the second race-loser sees `false` and gets a 409.
    pub async fn try_set_initial_web_password(&self, argon2_hash: &str) -> WebAuthResult<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO web_password (id, argon2_hash, updated_at) VALUES (1, ?, ?)",
        )
        .bind(argon2_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically create the first web password and its initial device
    /// token binding. Returns `true` iff no password existed before this
    /// call; a race-loser returns `false` without creating a device.
    ///
    /// If the device insert fails, this explicitly rolls back the password
    /// insert before returning the error, so an operator is never left with
    /// a password-only onboarding state.
    pub async fn try_set_initial_web_password_and_device(
        &self,
        argon2_hash: &str,
        id: &str,
        name: &str,
        token_hash: &str,
    ) -> WebAuthResult<bool> {
        let mut tx = self.pool.begin().await.map_err(WebAuthError::from_sqlx)?;
        let result: WebAuthResult<bool> = async {
            let password_insert = sqlx::query(
                "INSERT OR IGNORE INTO web_password (id, argon2_hash, updated_at) VALUES (1, ?, ?)",
            )
            .bind(argon2_hash)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;

            if password_insert.rows_affected() == 0 {
                return Ok(false);
            }

            sqlx::query(
                "INSERT INTO web_devices (id, name, token_hash, created_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(name)
            .bind(token_hash)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;

            Ok(true)
        }
        .await;

        match result {
            Ok(created) => {
                tx.commit().await.map_err(WebAuthError::from_sqlx)?;
                Ok(created)
            }
            Err(error) => {
                tx.rollback().await.map_err(WebAuthError::from_sqlx)?;
                Err(error)
            }
        }
    }

    /// Fetch the stored web password hash, if one has been set.
    pub async fn get_web_password_hash(&self) -> WebAuthResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT argon2_hash FROM web_password WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(WebAuthError::from_sqlx)?;
        Ok(row.map(|(h,)| h))
    }

    /// Wipe the password + all devices + passkeys (reset). The audit rows
    /// stay so the operator can see the reset event.
    ///
    /// Passkey rows would be reaped by the `ON DELETE CASCADE` on
    /// `web_passkeys.device_id` once `web_devices` is deleted, but we delete
    /// them explicitly first so the transaction is resilient to an operator
    /// running against an older DB where `foreign_keys=OFF` happened to be
    /// the default.
    pub async fn reset_web_password(&self) -> WebAuthResult<()> {
        let mut tx = self.pool.begin().await.map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_passkeys")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_devices")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        sqlx::query("DELETE FROM web_password")
            .execute(&mut *tx)
            .await
            .map_err(WebAuthError::from_sqlx)?;
        tx.commit().await.map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Record a new device token binding.
    ///
    /// INVARIANT — caller MUST pass:
    ///   - `id`: a UUIDv4 string (never reused across a reset)
    ///   - `token_hash`: hex-encoded sha256 of a raw bearer ≥32 random bytes
    ///     (base64url-encoded). The raw token must never reach this crate —
    ///     storing a hash means a `wisphive.db` leak does not yield usable
    ///     credentials.
    ///
    /// Returns `Duplicate` if either `id` or `token_hash` already exists.
    pub async fn insert_web_device(
        &self,
        id: &str,
        name: &str,
        token_hash: &str,
    ) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_devices (id, name, token_hash, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(token_hash)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Find a non-revoked device by its token hash. Also returns the device
    /// name so callers can populate the request context.
    ///
    /// Relies on the `UNIQUE` constraint on `token_hash` for the "at most
    /// one match" invariant; `LIMIT 1` is a belt-and-suspenders guard.
    pub async fn find_web_device_by_token_hash(
        &self,
        token_hash: &str,
    ) -> WebAuthResult<Option<WebDeviceRow>> {
        let row: Option<WebDeviceActiveRow> = sqlx::query_as(
            "SELECT id, name, created_at, last_seen_at, last_ip
             FROM web_devices
             WHERE token_hash = ? AND revoked_at IS NULL
             LIMIT 1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(row.map(
            |(id, name, created_at, last_seen_at, last_ip)| WebDeviceRow {
                id,
                name,
                created_at,
                last_seen_at,
                last_ip,
                revoked_at: None,
            },
        ))
    }

    /// Flip `revoked_at` on a device, idempotently. A second call is a
    /// no-op because the WHERE clause filters already-revoked rows.
    pub async fn revoke_web_device(&self, id: &str) -> WebAuthResult<()> {
        sqlx::query(
            "UPDATE web_devices SET revoked_at = ?
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Record that we've just served a request on behalf of `device_id`.
    /// Best-effort: callers should fire-and-forget.
    ///
    /// Only touches non-revoked devices so post-revocation forensics stay
    /// clean (a revoked device's `last_seen_at` is frozen at the moment of
    /// its last legitimate use).
    pub async fn touch_web_device(&self, id: &str, ip: Option<&str>) -> WebAuthResult<()> {
        sqlx::query(
            "UPDATE web_devices SET last_seen_at = ?, last_ip = ?
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(ip)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// List all devices, newest first. Includes revoked so the UI can show
    /// history.
    pub async fn list_web_devices(&self) -> WebAuthResult<Vec<WebDeviceRow>> {
        let rows: Vec<WebDeviceFullRow> = sqlx::query_as(
            "SELECT id, name, created_at, last_seen_at, last_ip, revoked_at
                 FROM web_devices
                 ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, name, created_at, last_seen_at, last_ip, revoked_at)| WebDeviceRow {
                    id,
                    name,
                    created_at,
                    last_seen_at,
                    last_ip,
                    revoked_at,
                },
            )
            .collect())
    }

    /// Append a row to the audit log. `detail` is typically JSON; anything
    /// over 4KB is truncated so a LAN attacker hammering /login cannot
    /// inflate the DB with unbounded attacker-controlled payloads.
    pub async fn append_web_audit(
        &self,
        event: &str,
        device_id: Option<&str>,
        ip: Option<&str>,
        detail: Option<&str>,
    ) -> WebAuthResult<()> {
        const MAX_DETAIL: usize = 4096;
        let detail = detail.map(|d| {
            if d.len() > MAX_DETAIL {
                // Truncate at a char boundary to keep the row as valid UTF-8.
                let mut cut = MAX_DETAIL;
                while !d.is_char_boundary(cut) {
                    cut -= 1;
                }
                &d[..cut]
            } else {
                d
            }
        });
        sqlx::query(
            "INSERT INTO web_audit (at, event, device_id, ip, detail)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(event)
        .bind(device_id)
        .bind(ip)
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// Query recent audit rows, newest first. Limit is clamped at 1000 so a
    /// misbehaving caller cannot force SQLite to materialize the whole
    /// table.
    pub async fn list_web_audit(&self, limit: u32) -> WebAuthResult<Vec<WebAuditRow>> {
        let clamped = limit.min(1000);
        let rows: Vec<WebAuditRowRaw> = sqlx::query_as(
            "SELECT id, at, event, device_id, ip, detail
                 FROM web_audit ORDER BY id DESC LIMIT ?",
        )
        .bind(clamped)
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|(id, at, event, device_id, ip, detail)| WebAuditRow {
                id,
                at,
                event,
                device_id,
                ip,
                detail,
            })
            .collect())
    }
}

#[cfg(test)]
#[path = "web_auth_tests.rs"]
mod tests;
