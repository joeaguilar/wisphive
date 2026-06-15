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
