//! Purpose: error envelope JSON parity (tag first, optional keys omitted, statuses).
//! Scope: pure, fixture-free.
//! Uses:
//! rust/crates/
//! `-- protocol/src/errors.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: bytes match `Schema.TaggedErrorClass` encoding.
//! Contract tests: protocol/tests/errors.rs

use opencode_protocol::errors::ApiError;

#[test]
fn envelopes_match_baseline_bytes() {
    let e = ApiError::SessionNotFoundError {
        session_id: "ses_1".into(),
        message: "nope".into(),
    };
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"_tag":"SessionNotFoundError","sessionID":"ses_1","message":"nope"}"#
    );
    assert_eq!(e.status(), 404);
    assert_eq!(
        serde_json::to_string(&ApiError::unauthorized()).unwrap(),
        r#"{"_tag":"UnauthorizedError","message":"Authentication required"}"#
    );
    let long = "x".repeat(1030);
    let ApiError::InvalidRequestError { message, .. } = ApiError::schema_rejection(&long, "Decode")
    else {
        panic!()
    };
    assert!(message.ends_with("... (6 more chars)"));
}
