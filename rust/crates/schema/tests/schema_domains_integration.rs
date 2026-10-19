//! Purpose: verify integration, connection, and credential values against Bun captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! `-- schema/src/integration.rs
//! External:
//! |-- serde::Serialize
//! |-- serde::de::DeserializeOwned
//! `-- serde_json::from_str
//! Used by:
//! `-- none, entrypoint
//! Invariants: all tagged unions have exact type and field order.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_integration

use opencode_schema::integration::{
    AttemptStatus, ConnectionInfo, CredentialValue, Info, Method, Prompt, WhenOp,
};

fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

#[test]
fn fixture_captured_from_bun_at_baseline() {
    // fixture: captured from bun at baseline
    roundtrip::<Prompt>(
        r#"{"type":"text","key":"k","message":"m","placeholder":"p","when":{"key":"wk","op":"eq","value":"wv"}}"#,
    );
    roundtrip::<Prompt>(
        r#"{"type":"select","key":"k","message":"m","options":[{"label":"l","value":"v","hint":"h"}]}"#,
    );
    roundtrip::<Method>(r#"{"id":"m1","type":"oauth","label":"OAuth"}"#);
    roundtrip::<Method>(r#"{"type":"key","label":"Key"}"#);
    roundtrip::<Method>(r#"{"type":"env","names":["VAR1","VAR2"]}"#);
    roundtrip::<ConnectionInfo>(r#"{"type":"credential","id":"cred_1","label":"My Cred"}"#);
    roundtrip::<ConnectionInfo>(r#"{"type":"env","name":"MY_VAR"}"#);
    roundtrip::<CredentialValue>(
        r#"{"type":"oauth","methodID":"m1","refresh":"r","access":"a","expires":3600}"#,
    );
    roundtrip::<CredentialValue>(r#"{"type":"key","key":"secret","metadata":{"foo":"bar"}}"#);
    roundtrip::<AttemptStatus>(r#"{"status":"pending","time":{"created":1000,"expires":2000}}"#);
    roundtrip::<AttemptStatus>(
        r#"{"status":"failed","message":"error","time":{"created":1000,"expires":2000}}"#,
    );
}

#[test]
fn optional_omission_and_null_rejection() {
    let prompt: Prompt =
        serde_json::from_str(r#"{"type":"text","key":"k","message":"m"}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&prompt).unwrap(),
        r#"{"type":"text","key":"k","message":"m"}"#
    );
    assert!(
        serde_json::from_str::<Prompt>(
            r#"{"type":"text","key":"k","message":"m","placeholder":null}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<Prompt>(r#"{"type":"text","key":"k","message":"m","when":null}"#)
            .is_err()
    );

    let method: Method = serde_json::from_str(r#"{"type":"key"}"#).unwrap();
    assert_eq!(serde_json::to_string(&method).unwrap(), r#"{"type":"key"}"#);
    assert!(serde_json::from_str::<Method>(r#"{"type":"key","label":null}"#).is_err());

    let cred: CredentialValue = serde_json::from_str(r#"{"type":"key","key":"secret"}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&cred).unwrap(),
        r#"{"type":"key","key":"secret"}"#
    );
    assert!(
        serde_json::from_str::<CredentialValue>(r#"{"type":"key","key":"secret","metadata":null}"#)
            .is_err()
    );
}

#[test]
fn required_fields_and_invalid_defaults() {
    assert!(serde_json::from_str::<Prompt>(r#"{"type":"text"}"#).is_err());
    assert!(serde_json::from_str::<Prompt>(r#"{"type":"select"}"#).is_err());
    assert!(serde_json::from_str::<Method>(r#"{"type":"oauth"}"#).is_err());
    assert!(serde_json::from_str::<Method>(r#"{"type":"env"}"#).is_err());
    assert!(serde_json::from_str::<ConnectionInfo>(r#"{"type":"credential"}"#).is_err());
    assert!(serde_json::from_str::<ConnectionInfo>(r#"{"type":"env"}"#).is_err());
    assert!(serde_json::from_str::<CredentialValue>(r#"{"type":"oauth"}"#).is_err());
    assert!(serde_json::from_str::<CredentialValue>(r#"{"type":"key"}"#).is_err());
    assert!(serde_json::from_str::<AttemptStatus>(r#"{"status":"pending"}"#).is_err());
    assert!(serde_json::from_str::<AttemptStatus>(r#"{"status":"failed"}"#).is_err());

    assert!(
        serde_json::from_str::<Prompt>(r#"{"type":"invalid","key":"k","message":"m"}"#).is_err()
    );
    assert!(serde_json::from_str::<Method>(r#"{"type":"invalid","label":"L"}"#).is_err());
    assert!(serde_json::from_str::<ConnectionInfo>(r#"{"type":"invalid"}"#).is_err());
    assert!(serde_json::from_str::<CredentialValue>(r#"{"type":"invalid"}"#).is_err());
    assert!(
        serde_json::from_str::<AttemptStatus>(
            r#"{"status":"invalid","time":{"created":1,"expires":2}}"#
        )
        .is_err()
    );
}

#[test]
fn tagged_union_order_and_structure() {
    let info: Info = serde_json::from_str(
        r#"{"id":"int_1","name":"Test","methods":[{"type":"key"}],"connections":[{"type":"env","name":"X"}]}"#,
    )
    .unwrap();
    assert_eq!(info.id.as_str(), "int_1");
    assert_eq!(info.name, "Test");
    assert_eq!(info.methods.len(), 1);
    assert_eq!(info.connections.len(), 1);

    let status: AttemptStatus =
        serde_json::from_str(r#"{"status":"complete","time":{"created":10,"expires":20}}"#)
            .unwrap();
    match status {
        AttemptStatus::Complete { time } => {
            assert_eq!(time.created, 10);
            assert_eq!(time.expires, 20);
        }
        _ => panic!("Expected Complete"),
    }

    let cred: CredentialValue = serde_json::from_str(
        r#"{"type":"oauth","methodID":"m","refresh":"r","access":"a","expires":0}"#,
    )
    .unwrap();
    match cred {
        CredentialValue::OAuth {
            method_id, expires, ..
        } => {
            assert_eq!(method_id.as_str(), "m");
            assert_eq!(expires.get(), 0);
        }
        _ => panic!("Expected OAuth"),
    }
}

#[test]
fn when_op_serializes_lowercase() {
    let when = opencode_schema::integration::When {
        key: "k".to_string(),
        op: WhenOp::Eq,
        value: "v".to_string(),
    };
    assert_eq!(
        serde_json::to_string(&when).unwrap(),
        r#"{"key":"k","op":"eq","value":"v"}"#
    );
    let decoded: opencode_schema::integration::When =
        serde_json::from_str(r#"{"key":"k","op":"neq","value":"v"}"#).unwrap();
    assert_eq!(decoded.op, WhenOp::Neq);
}
