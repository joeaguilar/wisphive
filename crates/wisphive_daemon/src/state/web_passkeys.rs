use super::{StateDb, WebAuthError, WebAuthResult};

/// Row shape for `web_passkeys` queries.
///
/// Columns in declaration order: `id`, `device_id`, `public_key`, `sign_count`,
/// `transports`, `created_at`, `last_used_at`, `aaguid`, `rp_id`. The
/// trailing two were added by itr#311 alongside the WebAuthn handlers; new
/// inserts populate both, older rows produced before the migration carry
/// `aaguid IS NULL` + `rp_id = ''` (the latter is what
/// `wisphive_web::auth_profile::scan_passkey_rp_id_drift` keys off for
/// "re-enroll required" warnings).
type WebPasskeyRowRaw = (
    String,
    String,
    Vec<u8>,
    i64,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

/// A row from `web_passkeys`.
///
/// The `aaguid` + `rp_id` columns were added by itr#311's WebAuthn
/// handler PR; older rows produced before the migration carry
/// `aaguid IS NULL` + `rp_id = ""`. `wisphive_web::auth_profile::scan_passkey_rp_id_drift`
/// keys off the empty-string sentinel at startup to warn the operator
/// that those credentials need to be re-enrolled under the active profile.
#[derive(Debug, Clone)]
pub struct WebPasskeyRow {
    pub id: String,
    pub device_id: String,
    pub public_key: Vec<u8>,
    pub sign_count: i64,
    pub transports: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    /// Authenticator AAGUID, if the device exposed one. Many synced
    /// passkeys (iCloud, Google Password Manager) return all-zeros or
    /// omit the field; we store it raw so future "pretty-name the
    /// authenticator" UI can look it up without re-parsing the credential
    /// blob. Stored as `TEXT` to keep the column human-readable in
    /// `sqlite3` shells.
    pub aaguid: Option<String>,
    /// WebAuthn RP ID under which this credential was enrolled. Empty
    /// string for pre-migration rows.
    pub rp_id: String,
}

impl StateDb {
    /// Persist a newly enrolled passkey. Returns `Duplicate` if the
    /// credential id is already enrolled.
    ///
    /// `aaguid` is the raw authenticator AAGUID (16 bytes formatted as a
    /// UUID / base64url / hex string — pick a representation in the
    /// caller and be consistent). Many synced passkeys (iCloud, Google
    /// Password Manager) omit it or return all-zeros; pass `None` in
    /// that case rather than substituting a placeholder.
    ///
    /// `rp_id` is the WebAuthn RP ID under which the credential was
    /// enrolled. Drives the profile-switch warning in
    /// `wisphive_web::auth_profile::scan_passkey_rp_id_drift`. Pass `""`
    /// from tests / fixtures where the value isn't meaningful — that
    /// matches the migration default for pre-#311 rows.
    ///
    /// Clippy nags about the 8-argument count; every value here is a
    /// distinct column on a single table and a struct-based wrapper
    /// would still serialize to the same eight bind sites. The cleanup
    /// is wholly cosmetic — accepted as-is.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_web_passkey(
        &self,
        id: &str,
        device_id: &str,
        public_key: &[u8],
        sign_count: i64,
        transports_json: Option<&str>,
        aaguid: Option<&str>,
        rp_id: &str,
    ) -> WebAuthResult<()> {
        sqlx::query(
            "INSERT INTO web_passkeys (id, device_id, public_key, sign_count, transports, created_at, aaguid, rp_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(device_id)
        .bind(public_key)
        .bind(sign_count)
        .bind(transports_json)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(aaguid)
        .bind(rp_id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }

    /// List all passkeys bound to a given device.
    pub async fn list_web_passkeys_for_device(
        &self,
        device_id: &str,
    ) -> WebAuthResult<Vec<WebPasskeyRow>> {
        let rows: Vec<WebPasskeyRowRaw> = sqlx::query_as(
            "SELECT id, device_id, public_key, sign_count, transports, created_at, last_used_at, aaguid, rp_id
                 FROM web_passkeys
                 WHERE device_id = ?
                 ORDER BY created_at DESC",
        )
        .bind(device_id)
        .fetch_all(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    device_id,
                    public_key,
                    sign_count,
                    transports,
                    created_at,
                    last_used_at,
                    aaguid,
                    rp_id,
                )| WebPasskeyRow {
                    id,
                    device_id,
                    public_key,
                    sign_count,
                    transports,
                    created_at,
                    last_used_at,
                    aaguid,
                    rp_id,
                },
            )
            .collect())
    }

    /// Look up a single passkey by its credential id. Used by
    /// `POST /api/auth/passkey/login/finish` (itr#311) to resolve the
    /// originating device after `webauthn-rs::Webauthn::finish_discoverable_authentication`
    /// hands us a credential ID — discoverable login flows don't know
    /// which device they're authenticating until after the user picks a
    /// credential, which is exactly the lookup this method backs.
    pub async fn find_web_passkey_by_credential_id(
        &self,
        credential_id: &str,
    ) -> WebAuthResult<Option<WebPasskeyRow>> {
        let row: Option<WebPasskeyRowRaw> = sqlx::query_as(
            "SELECT id, device_id, public_key, sign_count, transports, created_at, last_used_at, aaguid, rp_id
                 FROM web_passkeys
                 WHERE id = ?
                 LIMIT 1",
        )
        .bind(credential_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(row.map(
            |(
                id,
                device_id,
                public_key,
                sign_count,
                transports,
                created_at,
                last_used_at,
                aaguid,
                rp_id,
            )| WebPasskeyRow {
                id,
                device_id,
                public_key,
                sign_count,
                transports,
                created_at,
                last_used_at,
                aaguid,
                rp_id,
            },
        ))
    }

    /// Bump the sign counter and refresh `last_used_at` after a
    /// successful passkey authentication. Called by
    /// `POST /api/auth/passkey/login/finish` once `webauthn-rs::Webauthn::finish_discoverable_authentication`
    /// has verified the assertion AND the caller has confirmed
    /// `new_sign_count > stored_sign_count` (rejecting equal-or-lower
    /// counts is mandated by WebAuthn §7.2 step 21 — a cloned credential
    /// will replay the lower counter).
    pub async fn update_passkey_sign_count_and_last_used(
        &self,
        credential_id: &str,
        new_sign_count: i64,
    ) -> WebAuthResult<()> {
        sqlx::query(
            "UPDATE web_passkeys
             SET sign_count = ?, last_used_at = ?
             WHERE id = ?",
        )
        .bind(new_sign_count)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(credential_id)
        .execute(&self.pool)
        .await
        .map_err(WebAuthError::from_sqlx)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::state::test_support::test_db;

    #[tokio::test]
    async fn web_passkey_insert_and_list_cascade_deletes() {
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.insert_web_passkey(
            "pk-a",
            "dev-1",
            b"cose-a",
            0,
            Some("[\"internal\"]"),
            Some("aaguid-a"),
            "localhost",
        )
        .await
        .unwrap();
        db.insert_web_passkey("pk-b", "dev-1", b"cose-b", 0, None, None, "localhost")
            .await
            .unwrap();

        let keys = db.list_web_passkeys_for_device("dev-1").await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|k| k.id == "pk-a"));
        assert!(keys.iter().any(|k| k.id == "pk-b"));
        assert_eq!(
            keys.iter().find(|k| k.id == "pk-a").unwrap().public_key,
            b"cose-a"
        );

        // ON DELETE CASCADE kicks in when the device row is removed (via reset).
        db.reset_web_password().await.unwrap();
        assert!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn web_device_fk_cascade_drops_passkeys_when_device_row_is_deleted() {
        // Regression guard for the FK-off footgun: enabling
        // `foreign_keys=ON` at connect time makes the CASCADE actually fire.
        // If someone ever turns the pragma off the cascade test in
        // `web_passkey_insert_and_list_cascade_deletes` would still pass
        // (because reset_web_password deletes passkeys manually first), but
        // this one will not.
        let db = test_db().await;
        db.insert_web_device("dev-1", "phone", "hash-1")
            .await
            .unwrap();
        db.insert_web_passkey("pk-1", "dev-1", b"cose", 0, None, None, "")
            .await
            .unwrap();
        assert_eq!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .len(),
            1
        );

        sqlx::query("DELETE FROM web_devices WHERE id = ?")
            .bind("dev-1")
            .execute(db.pool())
            .await
            .unwrap();

        assert!(
            db.list_web_passkeys_for_device("dev-1")
                .await
                .unwrap()
                .is_empty(),
            "ON DELETE CASCADE must reap passkeys when foreign_keys=ON"
        );
    }
}
