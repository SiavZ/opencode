//! Purpose: conformance tests for credentials (packages/core/src/credential.ts) and legacy provider
//! auth (packages/opencode/src/auth/index.ts, cases of packages/opencode/test/auth/auth.test.ts:9-75).
//! Scope: live tests on a migrated in-memory `SqliteDatabase` and on temp data dirs.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- serde_json
//! |-- tempfile
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: never reads the real user data dir or the process `OPENCODE_AUTH_CONTENT`.
//! Contract tests: storage-sqlite/tests/credentials.rs

use opencode_storage_api::{DatabaseExt, Sql, StorageError, TxOptions, Value};
use opencode_storage_sqlite::SqliteDatabase;
use opencode_storage_sqlite::legacy_files::{AuthInfo, LegacyAuth};
use opencode_storage_sqlite::repositories::credentials::{self, CredentialValue};
use serde_json::json;

type R<T> = Result<T, StorageError>;

async fn tx<T: Send + 'static>(
    db: &SqliteDatabase,
    work: impl FnOnce(&dyn Sql) -> R<T> + Send + 'static,
) -> R<T> {
    db.transaction(TxOptions::default(), work).await
}

fn key(k: &str) -> CredentialValue {
    CredentialValue::Key {
        key: k.into(),
        metadata: None,
    }
}

fn wellknown(token: &str) -> AuthInfo {
    AuthInfo::WellKnown {
        key: "TOKEN".into(),
        token: token.into(),
    }
}

// create replaces any credential of the integration (rotation), labels default to "default".
#[tokio::test]
async fn create_rotates_per_integration() {
    let db = SqliteDatabase::open_in_memory().await.unwrap();
    let (first, second, other, listed, all) = tx(&db, |sql| {
        let first = credentials::create(sql, "github", &key("k1"), None)?;
        let second = credentials::create(sql, "github", &key("k2"), Some("work"))?;
        let other = credentials::create(sql, "openai", &key("k3"), None)?;
        Ok((
            first,
            second,
            other,
            credentials::list(sql, "github")?,
            credentials::all(sql)?,
        ))
    })
    .await
    .unwrap();
    assert!(first.id.as_str().starts_with("cred_"));
    assert_eq!(first.label, "default");
    assert_eq!(listed, std::slice::from_ref(&second));
    assert_eq!(listed[0].label, "work");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1], other);
    let read = tx(&db, move |sql| credentials::get(sql, first.id.as_str()))
        .await
        .unwrap();
    assert_eq!(read, None);
}

// Stored value is compact JSON of the tagged union in schema field order; legacy columns are NULL.
#[tokio::test]
async fn stored_encoding_matches_ts() {
    let db = SqliteDatabase::open_in_memory().await.unwrap();
    let value = CredentialValue::Oauth {
        method_id: "device".into(),
        refresh: "r".into(),
        access: "a".into(),
        expires: 5,
        metadata: Some(json!({"scope": "x"}).as_object().unwrap().clone()),
    };
    let row = tx(&db, move |sql| {
        credentials::create(sql, "github", &value, None)?;
        Ok(sql
            .query_row(
                "SELECT value, connector_id, method_id, active FROM credential",
                &[],
            )?
            .unwrap()
            .into_values())
    })
    .await
    .unwrap();
    assert_eq!(
        row,
        [
            Value::Text(
                r#"{"type":"oauth","methodID":"device","refresh":"r","access":"a","expires":5,"metadata":{"scope":"x"}}"#
                    .into()
            ),
            Value::Null,
            Value::Null,
            Value::Null
        ]
    );
}

// Rows without integration_id are invisible; ordering is time_created ascending.
#[tokio::test]
async fn hides_legacy_rows_and_orders_by_creation() {
    let db = SqliteDatabase::open_in_memory().await.unwrap();
    let all = tx(&db, |sql| {
        sql.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES
             ('cred_legacy', NULL, 'old', '{\"type\":\"key\",\"key\":\"x\"}', 1, 1, 1),
             ('cred_b', 'b', 'default', '{\"type\":\"key\",\"key\":\"b\"}', NULL, 3, 3),
             ('cred_a', 'a', 'default', '{\"type\":\"key\",\"key\":\"a\"}', NULL, 2, 2)",
            &[],
        )?;
        Ok((credentials::all(sql)?, credentials::get(sql, "cred_legacy")?))
    })
    .await
    .unwrap();
    let ids: Vec<&str> = all.0.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["cred_a", "cred_b"]);
    assert_eq!(all.1, None);
}

// update: neither label nor value is a no-op; partial updates leave the rest; bad JSON is a Decode error.
#[tokio::test]
async fn update_remove_and_decode_failure() {
    let db = SqliteDatabase::open_in_memory().await.unwrap();
    let (unchanged, relabeled, revalued, removed) = tx(&db, |sql| {
        let info = credentials::create(sql, "github", &key("k1"), None)?;
        let id = info.id.as_str().to_owned();
        credentials::update(sql, &id, None, None)?;
        credentials::update(sql, &id, Some(""), None)?;
        let unchanged = credentials::get(sql, &id)?.unwrap();
        credentials::update(sql, &id, Some("renamed"), None)?;
        let relabeled = credentials::get(sql, &id)?.unwrap();
        credentials::update(sql, &id, None, Some(&key("k2")))?;
        let revalued = credentials::get(sql, &id)?.unwrap();
        credentials::remove(sql, &id)?;
        Ok((unchanged, relabeled, revalued, credentials::get(sql, &id)?))
    })
    .await
    .unwrap();
    assert_eq!(unchanged.label, "default");
    assert_eq!(relabeled.label, "renamed");
    assert_eq!(
        (revalued.label.as_str(), revalued.value.expose()),
        ("renamed", &key("k2"))
    );
    assert_eq!(removed, None);

    let error = tx(&db, |sql| {
        sql.execute(
            "INSERT INTO credential (id, integration_id, label, value, time_created, time_updated) VALUES ('cred_bad', 'x', 'l', '{\"type\":\"nope\"}', 1, 1)",
            &[],
        )?;
        credentials::all(sql)
    })
    .await
    .unwrap_err();
    assert!(matches!(error, StorageError::Decode { ref column, .. } if column == "value"));
}

#[test]
fn credential_debug_redacts_secrets() {
    let value = CredentialValue::Oauth {
        method_id: "m".into(),
        refresh: "refresh-secret".into(),
        access: "access-secret".into(),
        expires: 1,
        metadata: None,
    };
    let text = format!("{value:?} {:?}", key("sk-live-123"));
    assert!(
        !text.contains("secret") && !text.contains("sk-live"),
        "{text}"
    );
}

// auth.test.ts "set normalizes trailing slashes in keys"
#[tokio::test]
async fn auth_set_normalizes_trailing_slash() {
    let dir = tempfile::tempdir().unwrap();
    let auth = LegacyAuth::new(dir.path(), None);
    auth.set("https://example.com/", &wellknown("abc"))
        .await
        .unwrap();
    let data = auth.all().await.unwrap();
    assert!(data.contains_key("https://example.com"));
    assert!(!data.contains_key("https://example.com/"));
}

// auth.test.ts "set cleans up pre-existing trailing-slash entry"
#[tokio::test]
async fn auth_set_cleans_trailing_slash_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.json"),
        r#"{"https://example.com/":{"type":"wellknown","key":"TOKEN","token":"old"}}"#,
    )
    .unwrap();
    let auth = LegacyAuth::new(dir.path(), None);
    auth.set("https://example.com", &wellknown("new"))
        .await
        .unwrap();
    let data = auth.all().await.unwrap();
    let keys: Vec<&String> = data.keys().filter(|k| k.contains("example.com")).collect();
    assert_eq!(keys, ["https://example.com"]);
    assert_eq!(
        auth.get("https://example.com").await.unwrap(),
        Some(wellknown("new"))
    );
}

// auth.test.ts "remove deletes both trailing-slash and normalized keys"
#[tokio::test]
async fn auth_remove_deletes_both_forms() {
    let dir = tempfile::tempdir().unwrap();
    let auth = LegacyAuth::new(dir.path(), None);
    auth.set("https://example.com", &wellknown("abc"))
        .await
        .unwrap();
    auth.remove("https://example.com/").await.unwrap();
    let data = auth.all().await.unwrap();
    assert!(
        !data.contains_key("https://example.com") && !data.contains_key("https://example.com/")
    );
}

// auth.test.ts "set and remove are no-ops on keys without trailing slashes"
#[tokio::test]
async fn auth_plain_keys() {
    let dir = tempfile::tempdir().unwrap();
    let auth = LegacyAuth::new(dir.path(), None);
    let api = AuthInfo::Api {
        key: "sk-test".into(),
        metadata: None,
    };
    auth.set("anthropic", &api).await.unwrap();
    assert_eq!(auth.get("anthropic").await.unwrap(), Some(api));
    auth.remove("anthropic").await.unwrap();
    assert_eq!(auth.get("anthropic").await.unwrap(), None);
}

// writeJson(file, data, 0o600): 2-space JSON, mode 0600, invalid entries dropped and excess keys stripped.
#[tokio::test]
async fn auth_file_format_permissions_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.json"),
        r#"{"bad":{"type":"oauth","expires":-1,"access":"a","refresh":"r"},"keep":{"extra":1,"type":"oauth","expires":5.0,"access":"a","refresh":"r"},"junk":3}"#,
    )
    .unwrap();
    let auth = LegacyAuth::new(dir.path(), None);
    auth.set(
        "openai",
        &AuthInfo::Api {
            key: "k".into(),
            metadata: None,
        },
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(dir.path().join("auth.json")).unwrap();
    assert_eq!(
        text,
        "{\n  \"keep\": {\n    \"type\": \"oauth\",\n    \"refresh\": \"r\",\n    \"access\": \"a\",\n    \"expires\": 5\n  },\n  \"openai\": {\n    \"type\": \"api\",\n    \"key\": \"k\"\n  }\n}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!format!("{auth:?} {:?}", auth.get("keep").await.unwrap()).contains("\"a\""));
}

// OPENCODE_AUTH_CONTENT wins when it parses (returned unvalidated); invalid JSON falls back to the file.
#[tokio::test]
async fn auth_env_content_override() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.json"),
        r#"{"file":{"type":"api","key":"f"}}"#,
    )
    .unwrap();
    let env = LegacyAuth::new(
        dir.path(),
        Some(r#"{"env":{"type":"api","key":"e","x":1}}"#.into()),
    );
    assert_eq!(
        serde_json::Value::Object(env.all().await.unwrap()),
        json!({"env": {"type": "api", "key": "e", "x": 1}})
    );
    let broken = LegacyAuth::new(dir.path(), Some("{not json".into()));
    assert!(broken.all().await.unwrap().contains_key("file"));
    let empty = LegacyAuth::new(dir.path(), Some(String::new()));
    assert!(empty.all().await.unwrap().contains_key("file"));
}

// A missing data dir is not created: the write fails with the TS message.
#[tokio::test]
async fn auth_write_failure_message() {
    let dir = tempfile::tempdir().unwrap();
    let auth = LegacyAuth::new(dir.path().join("missing"), None);
    let error = auth.set("x", &wellknown("t")).await.unwrap_err();
    assert!(
        error.to_string().contains("Failed to write auth data"),
        "{error}"
    );
}
