use super::*;
use crate::state::test_support::test_db;

// ════════════════════════════════════════════════════════════
// Web auth helpers
// ════════════════════════════════════════════════════════════

#[tokio::test]
async fn web_password_set_get_and_reset() {
    let db = test_db().await;
    assert!(db.get_web_password_hash().await.unwrap().is_none());

    db.set_web_password("$argon2id$hash1").await.unwrap();
    assert_eq!(
        db.get_web_password_hash().await.unwrap().as_deref(),
        Some("$argon2id$hash1")
    );

    // Upsert overwrites.
    db.set_web_password("$argon2id$hash2").await.unwrap();
    assert_eq!(
        db.get_web_password_hash().await.unwrap().as_deref(),
        Some("$argon2id$hash2")
    );

    // Reset cascades devices/passkeys and clears the password.
    db.insert_web_device("dev-1", "phone", "tokhash-1")
        .await
        .unwrap();
    db.insert_web_passkey("pk-1", "dev-1", b"fake-key", 0, None, None, "")
        .await
        .unwrap();
    db.reset_web_password().await.unwrap();
    assert!(db.get_web_password_hash().await.unwrap().is_none());
    assert!(db.list_web_devices().await.unwrap().is_empty());
    assert!(
        db.list_web_passkeys_for_device("dev-1")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn web_password_rehash_compare_and_swap_preserves_newer_hash() {
    let db = test_db().await;
    let weak_hash = "$argon2id$weak";
    let first_upgrade = "$argon2id$upgrade-one";
    let stale_upgrade = "$argon2id$upgrade-two";
    db.set_web_password(weak_hash).await.unwrap();

    // Model two successful logins which both verified `weak_hash`. The first
    // replacement wins, while the second's stale compare-and-swap must not
    // clobber it with a different random-salt replacement.
    assert!(
        db.replace_web_password_hash_if_current(weak_hash, first_upgrade)
            .await
            .unwrap()
    );
    assert!(
        !db.replace_web_password_hash_if_current(weak_hash, stale_upgrade)
            .await
            .unwrap()
    );
    assert_eq!(
        db.get_web_password_hash().await.unwrap().as_deref(),
        Some(first_upgrade)
    );
}

#[tokio::test]
async fn initial_web_password_and_device_rolls_back_when_device_insert_fails() {
    let db = test_db().await;
    db.insert_web_device("existing-device", "phone", "existing-token")
        .await
        .unwrap();

    let err = db
        .try_set_initial_web_password_and_device(
            "$argon2id$hash",
            "new-device",
            "laptop",
            "existing-token",
        )
        .await
        .expect_err("duplicate device token must fail the initial provisioning transaction");
    assert!(
        matches!(err, WebAuthError::Duplicate),
        "expected Duplicate, got {err:?}"
    );
    assert!(
        db.get_web_password_hash().await.unwrap().is_none(),
        "failed initial device insertion must roll back the password"
    );
    assert_eq!(
        db.list_web_devices().await.unwrap().len(),
        1,
        "the failed transaction must not add another device"
    );

    // The rollback must leave the pooled connection usable for a later,
    // legitimate first-run provisioning attempt.
    assert!(
        db.try_set_initial_web_password_and_device(
            "$argon2id$hash",
            "new-device",
            "laptop",
            "new-token",
        )
        .await
        .unwrap()
    );
    assert_eq!(
        db.get_web_password_hash().await.unwrap().as_deref(),
        Some("$argon2id$hash")
    );
    assert_eq!(
        db.list_web_devices().await.unwrap().len(),
        2,
        "the retry must persist its initial device binding"
    );
}

#[tokio::test]
async fn web_device_insert_find_revoke_list() {
    let db = test_db().await;
    db.insert_web_device("dev-1", "phone", "hash-1")
        .await
        .unwrap();
    db.insert_web_device("dev-2", "laptop", "hash-2")
        .await
        .unwrap();

    let found = db
        .find_web_device_by_token_hash("hash-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, "dev-1");
    assert_eq!(found.name, "phone");

    // Touching updates last_seen/last_ip (smoke test).
    db.touch_web_device("dev-1", Some("192.168.1.5"))
        .await
        .unwrap();

    // Listing returns both; order is newest-first so dev-2 comes first.
    let devices = db.list_web_devices().await.unwrap();
    assert_eq!(devices.len(), 2);

    // Revoking hides the device from token lookups and flips revoked_at.
    db.revoke_web_device("dev-1").await.unwrap();
    assert!(
        db.find_web_device_by_token_hash("hash-1")
            .await
            .unwrap()
            .is_none()
    );
    let rev = db.list_web_devices().await.unwrap();
    let dev1 = rev.iter().find(|d| d.id == "dev-1").unwrap();
    assert!(dev1.revoked_at.is_some());

    // Revoking twice is a no-op.
    db.revoke_web_device("dev-1").await.unwrap();
}

#[tokio::test]
async fn web_device_token_hash_is_unique() {
    let db = test_db().await;
    db.insert_web_device("dev-1", "phone", "same-hash")
        .await
        .unwrap();
    let err = db
        .insert_web_device("dev-2", "laptop", "same-hash")
        .await
        .expect_err("second device with same token_hash must fail");
    assert!(
        matches!(err, WebAuthError::Duplicate),
        "expected Duplicate, got {err:?}"
    );
}

#[tokio::test]
async fn touch_web_device_ignores_revoked_rows() {
    let db = test_db().await;
    db.insert_web_device("dev-1", "phone", "hash-1")
        .await
        .unwrap();
    db.touch_web_device("dev-1", Some("10.0.0.1"))
        .await
        .unwrap();
    let before = db
        .list_web_devices()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.id == "dev-1")
        .unwrap();
    assert_eq!(before.last_ip.as_deref(), Some("10.0.0.1"));

    db.revoke_web_device("dev-1").await.unwrap();
    // Attempt to touch after revocation — must be a silent no-op.
    db.touch_web_device("dev-1", Some("10.0.0.99"))
        .await
        .unwrap();

    let after = db
        .list_web_devices()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.id == "dev-1")
        .unwrap();
    assert_eq!(
        after.last_ip.as_deref(),
        Some("10.0.0.1"),
        "revoked device's last_ip must be frozen"
    );
}

#[tokio::test]
async fn web_audit_append_and_list_newest_first() {
    let db = test_db().await;
    db.append_web_audit(
        "login_failure",
        None,
        Some("1.2.3.4"),
        Some("{\"reason\":\"bad_pw\"}"),
    )
    .await
    .unwrap();
    db.append_web_audit("login_success", Some("dev-1"), Some("1.2.3.4"), None)
        .await
        .unwrap();

    let rows = db.list_web_audit(10).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Newest first
    assert_eq!(rows[0].event, "login_success");
    assert_eq!(rows[0].device_id.as_deref(), Some("dev-1"));
    assert_eq!(rows[1].event, "login_failure");
    assert_eq!(rows[1].detail.as_deref(), Some("{\"reason\":\"bad_pw\"}"));
}

/// itr#258: every `append_web_audit` call in `wisphive_web` now formats a
/// non-`None` `detail` as JSON (`serde_json::json!({...}).to_string()`)
/// instead of a bare string, so future log consumers can parse the column
/// uniformly rather than guessing at a per-event convention. This exercises
/// one representative `detail` payload per event *kind* wisphive_web emits
/// — reason-code details (`web_password_set_denied`,
/// `web_device_revoke_denied`, `passkey_register_denied`,
/// `passkey_register_failure`, `passkey_login_failure`) and
/// value-carrying details (`web_device_revoke`'s target device id,
/// `passkey_register`/`passkey_login_success`'s credential id) — round-trips
/// through `append_web_audit` -> `list_web_audit` and confirms the stored
/// `detail` column parses as `serde_json::Value`.
#[tokio::test]
async fn web_audit_detail_is_json_for_every_event_kind() {
    let db = test_db().await;

    // Reason-code details: `{"reason": "<code>"}`.
    for (event, reason) in [
        ("web_password_set_denied", "password_too_long"),
        ("web_device_revoke_denied", "bad_password"),
        ("passkey_register_denied", "sudo_required"),
        ("passkey_register_failure", "webauthn_finish_error"),
        ("passkey_login_failure", "counter_regression"),
    ] {
        db.append_web_audit(
            event,
            Some("dev-1"),
            Some("1.2.3.4"),
            Some(&serde_json::json!({ "reason": reason }).to_string()),
        )
        .await
        .unwrap();
    }

    // Value-carrying details: a keyed JSON object, not a bare id string.
    db.append_web_audit(
        "web_device_revoke",
        Some("dev-1"),
        Some("1.2.3.4"),
        Some(&serde_json::json!({ "target_device_id": "dev-2" }).to_string()),
    )
    .await
    .unwrap();
    db.append_web_audit(
        "passkey_register",
        Some("dev-1"),
        Some("1.2.3.4"),
        Some(&serde_json::json!({ "credential_id": "cred-abc" }).to_string()),
    )
    .await
    .unwrap();
    db.append_web_audit(
        "passkey_login_success",
        Some("dev-1"),
        Some("1.2.3.4"),
        Some(&serde_json::json!({ "credential_id": "cred-abc" }).to_string()),
    )
    .await
    .unwrap();

    // `None`-detail events (e.g. `web_login_success`, `web_logout`) carry no
    // detail at all — nothing to parse, so they're outside this test's scope.

    let rows = db.list_web_audit(10).await.unwrap();
    assert_eq!(rows.len(), 8, "expected one row per append above");
    for row in &rows {
        let detail = row
            .detail
            .as_deref()
            .unwrap_or_else(|| panic!("event {:?} expected a detail column", row.event));
        let parsed: serde_json::Value = serde_json::from_str(detail).unwrap_or_else(|e| {
            panic!(
                "event {:?} detail {detail:?} did not parse as JSON: {e}",
                row.event
            )
        });
        assert!(
            parsed.is_object(),
            "event {:?} detail {detail:?} parsed but is not a JSON object",
            row.event
        );
    }
}
