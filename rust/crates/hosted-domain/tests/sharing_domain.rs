//! Purpose: fixture tests for hosted-domain sharing rules (collation, keyed merge, short IDs,
//! legacy import cursor, public DO key rules and share_data assembly).
//! Scope: fixture (expected orders captured from bun 1.3.14 `localeCompare` and the baseline TS).
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/sharing/collation.rs
//! |-- hosted-domain/src/sharing/mod.rs
//! |-- hosted-domain/src/sharing/public.rs
//! `-- hosted-domain/src/sharing/share.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_string
//! Used by:
//! `-- none, entrypoint
//! Invariants: no I/O.
//! Contract tests: this file

use opencode_hosted_domain::sharing::ShareError;
use opencode_hosted_domain::sharing::collation::{ASCII_ORDER, locale_compare};
use opencode_hosted_domain::sharing::public::{
    Entry, clear_plan, publish_allowed, replay, share_data, short_name,
};
use opencode_hosted_domain::sharing::share::{
    Compaction, DataKind, ShareData, key, last_utf16, legacy_step, merge, new_info, old_data_key,
};
use serde_json::{Value, json};

fn item(value: Value) -> ShareData {
    serde_json::from_value(value).expect("share data")
}

#[test]
fn collation_matches_bun_ascii_order() {
    let chars: Vec<char> = ASCII_ORDER.chars().collect();
    for pair in chars.windows(2) {
        let (a, b) = (pair[0].to_string(), pair[1].to_string());
        assert_eq!(
            locale_compare(&a, &b),
            std::cmp::Ordering::Less,
            "{a:?} < {b:?}"
        );
    }
    // Captured: "a-b".localeCompare("ab") = -1, "a_b" vs "a/b" = -1, "ab" vs "a-c" = 1.
    assert_eq!(locale_compare("a-b", "ab"), std::cmp::Ordering::Less);
    assert_eq!(locale_compare("a_b", "a/b"), std::cmp::Ordering::Less);
    assert_eq!(locale_compare("ab", "a-c"), std::cmp::Ordering::Greater);
}

#[test]
fn collation_sorts_share_keys_like_bun() {
    let mut keys = vec![
        "part/msg_B/prt_a",
        "part/msg_a/prt_b",
        "message/msg_Ab",
        "message/msg_aB",
        "message/msg_ab",
        "session_diff",
        "session",
        "model",
        "part/msg_a/prt_a",
        "message/msg_a1",
        "message/msg_a_",
        "message/msg_a-",
        "part/msg_a_x/prt_1",
        "part/msg_a/prt_1",
        "message/msg_0",
        "message/msg_A",
        "message/msg_a",
    ];
    keys.sort_by(|a, b| locale_compare(a, b));
    let expected = [
        "message/msg_0",
        "message/msg_a",
        "message/msg_A",
        "message/msg_a_",
        "message/msg_a-",
        "message/msg_a1",
        "message/msg_ab",
        "message/msg_aB",
        "message/msg_Ab",
        "model",
        "part/msg_a_x/prt_1",
        "part/msg_a/prt_1",
        "part/msg_a/prt_a",
        "part/msg_a/prt_b",
        "part/msg_B/prt_a",
        "session",
        "session_diff",
    ];
    assert_eq!(keys, expected);
}

#[test]
fn merge_is_latest_wins_and_sorted() {
    let first = [
        item(json!({"type": "part", "data": {"id": "p1", "messageID": "m1", "text": "old"}})),
        item(json!({"type": "session", "data": {"id": "s"}})),
    ];
    let second = [item(
        json!({"type": "part", "data": {"id": "p1", "messageID": "m1", "text": "new"}}),
    )];
    let merged = merge(&[&first, &second]).expect("merge");
    assert_eq!(merged.len(), 2);
    assert_eq!(
        merged[0].data.as_ref().and_then(|d| d.get("text")),
        Some(&json!("new"))
    );
    assert_eq!(merged[1].kind, DataKind::Session);
}

#[test]
fn key_uses_js_stringification() {
    assert_eq!(
        key(&item(json!({"type": "message", "data": {}}))).expect("key"),
        "message/undefined"
    );
    assert_eq!(
        key(&item(json!({"type": "message", "data": {"id": 7}}))).expect("key"),
        "message/7"
    );
    assert!(matches!(
        key(&item(json!({"type": "message"}))),
        Err(ShareError::Internal(_))
    ));
    let serialized = serde_json::to_string(&item(json!({"type": "model", "extra": 1, "data": []})))
        .expect("json");
    assert_eq!(serialized, r#"{"type":"model","data":[]}"#);
}

#[test]
fn share_ids_use_last_eight_utf16_units() {
    assert_eq!(
        new_info("ses_0123456789abcdefGHIJ", "s".into(), false).id,
        "cdefGHIJ"
    );
    assert_eq!(
        new_info("test_abcdefghij", "s".into(), false).id,
        "test_cdefghij"
    );
    assert_eq!(new_info("ses_x", "s".into(), true).id, "test_ses_x");
    assert_eq!(last_utf16("a😀b", 2), "\u{fffd}b");
    assert_eq!(short_name("ses_0123456789abcdefGHIJ"), "cdefGHIJ");
}

#[test]
fn legacy_cursor_is_the_oldest_listed_event() {
    let compaction = Compaction {
        event: Some("evt_002".into()),
        data: vec![],
    };
    let seg = |e: &str| vec!["share_event".to_string(), "x".to_string(), e.to_string()];
    let events = vec![
        (
            seg("evt_000"),
            Some(vec![item(json!({"type": "message", "data": {"id": "a"}}))]),
        ),
        (
            seg("evt_001"),
            Some(vec![item(
                json!({"type": "message", "data": {"id": "a", "v": 2}}),
            )]),
        ),
    ];
    let next = legacy_step(&compaction, &events)
        .expect("step")
        .expect("some");
    // Reversed processing makes the oldest event win and become the new cursor (share.ts:94,109).
    assert_eq!(next.event.as_deref(), Some("evt_000"));
    assert_eq!(next.data[0].data, Some(json!({"id": "a"})));
    assert!(legacy_step(&compaction, &[]).expect("empty").is_none());
}

#[test]
fn old_data_keys() {
    let part = item(json!({"type": "part", "data": {"id": "p", "messageID": "m"}}));
    assert_eq!(
        old_data_key("s1", &part).expect("key"),
        ["share_data", "s1", "part", "m", "p"]
    );
}

#[test]
fn public_publish_key_rules() {
    assert!(publish_allowed(Some("ses_1"), "session/info/ses_1"));
    assert!(publish_allowed(Some("ses_1"), "session/info/ses_1extra"));
    assert!(publish_allowed(Some("ses_1"), "session/part/ses_1/m/p"));
    assert!(!publish_allowed(Some("ses_1"), "session/message/ses_1"));
    assert!(!publish_allowed(Some("ses_1"), "session/info/ses_2"));
    assert!(publish_allowed(None, "session/info/undefined"));
    let plan = clear_plan(Some("ses_1"));
    assert_eq!(plan.list_prefix, "session/message/ses_1/");
    assert_eq!(plan.delete_info_key, "session/info/ses_1");
    assert_eq!(plan.list_limit, 1000);
}

#[test]
fn public_share_data_assembly() {
    let storage = vec![
        ("secret".to_string(), json!("x")),
        ("session/info/s".to_string(), json!({"id": "s"})),
        (
            "session/message/s/m1".to_string(),
            json!({"id": "m1", "role": "user"}),
        ),
        (
            "session/part/s/m1/p1".to_string(),
            json!({"id": "p1", "messageID": "m1"}),
        ),
        ("sessionID".to_string(), json!("s")),
    ];
    let entries = replay(&storage);
    assert_eq!(entries.len(), 3);
    let body = share_data(&entries).expect("assemble");
    assert_eq!(
        serde_json::to_string(&body).expect("json"),
        r#"{"info":{"id":"s"},"messages":{"m1":{"parts":[{"id":"p1","messageID":"m1"}],"id":"m1","role":"user"}}}"#
    );
    let orphan = [Entry {
        key: "session/part/s/m9/p".into(),
        content: json!({"messageID": "m9"}),
    }];
    assert!(share_data(&orphan).is_err());
    assert_eq!(
        serde_json::to_string(&share_data(&[]).expect("empty")).expect("json"),
        r#"{"messages":{}}"#
    );
}
