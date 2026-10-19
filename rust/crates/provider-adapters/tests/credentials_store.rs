//! Purpose: Verify canonical stored credential parsing and explicit environment fallback.
//! Scope: Public CredentialStore and CredentialValue APIs without process environment mutation.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/credentials/mod.rs
//! `-- schema/src/ids.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: stored values precede environment, failed loads are atomic, secrets stay redacted.
//! Contract tests: provider-adapters/tests/credentials_store.rs

use opencode_provider_adapters::credentials::{CredentialStore, CredentialValue};
use opencode_schema::ids::IntegrationID;
use serde_json::json;

#[test]
fn supplied_snapshot_is_the_only_environment_source() {
    let config =
        json!({"providers":{"custom":{"integrationID":"account","env":["EMPTY","SUPPLIED"]}}});
    let mut credentials = CredentialStore::from_config(&config).with_environment(
        [
            ("EMPTY".into(), "".into()),
            ("SUPPLIED".into(), "env-secret".into()),
        ]
        .into(),
    );
    let id = IntegrationID::new("account");
    assert_eq!(
        credentials.active_credential(&id).unwrap(),
        Some(CredentialValue::Key {
            key: "env-secret".into(),
            metadata: None
        })
    );
    assert_eq!(
        CredentialStore::from_config(&config)
            .active_credential(&id)
            .unwrap(),
        None
    );
    credentials.load_records(&json!([{"id":"cred_existing","integrationID":"account","label":"default","value":{"type":"key","key":"stored-secret","metadata":{"origin":"database"}}}])).unwrap();
    assert!(
        matches!(credentials.active_credential(&id).unwrap(), Some(CredentialValue::Key {key, metadata:Some(metadata)}) if key == "stored-secret" && metadata["origin"] == "database")
    );
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("stored-secret"));
    assert!(!debug.contains("env-secret"));
}

#[test]
fn canonical_oauth_and_sqlite_rows_preserve_material() {
    let mut credentials = CredentialStore::new();
    let oauth = json!({"type":"oauth","methodID":"browser","refresh":"refresh-secret","access":"access-secret","expires":123456789,"metadata":{"account":"tenant"}});
    credentials.load_records(&json!([
        {"integration_id":null,"value":"legacy invisible row"},
        {"integration_id":"account","value":oauth.to_string(),"label":"default","time_created":1}
    ])).unwrap();
    let value = credentials
        .active_credential(&IntegrationID::new("account"))
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&value).unwrap(), oauth);
    assert!(!format!("{value:?}").contains("secret"));
}

#[test]
fn invalid_records_are_atomic_and_errors_never_echo_secret_input() {
    let mut credentials = CredentialStore::new();
    let error = credentials
        .load_records(&json!([
            {"integrationID":"first","value":{"type":"key","key":"valid"}},
            {"integrationID":"second","value":{"type":"secret-invalid-tag","key":"sensitive"}}
        ]))
        .unwrap_err();
    assert!(!error.to_string().contains("secret-invalid-tag"));
    assert!(!error.to_string().contains("sensitive"));
    assert!(
        credentials
            .active_credential(&IntegrationID::new("first"))
            .unwrap()
            .is_none()
    );
    for value in [
        json!({"type":"oauth","methodID":"m","refresh":"r","access":"a","expires":-1}),
        json!({"type":"oauth","access":"a"}),
        json!({"type":"key"}),
    ] {
        assert!(
            credentials
                .load_records(&json!([{"integrationID":"bad","value":value}]))
                .is_err()
        );
    }
}
