//! Purpose: fixture tests (captured from jsonc-parser 3.3.1 and `ConfigParse.jsonc` in the TS
//! baseline) for JSONC parse values, error codes/offsets, error messages, and `modify` edits.
//! Scope: integration tests, fixture-based.
//! Uses:
//! rust/crates/
//! `-- config/src/jsonc/mod.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are regenerated only from the TS baseline, never hand edited.
//! Contract tests: config/tests/jsonc.rs

use opencode_config::jsonc::{self, ErrorCode};
use pretty_assertions::assert_eq;
use serde_json::Value;

const CODES: [ErrorCode; 16] = [
    ErrorCode::InvalidSymbol,
    ErrorCode::InvalidNumberFormat,
    ErrorCode::PropertyNameExpected,
    ErrorCode::ValueExpected,
    ErrorCode::ColonExpected,
    ErrorCode::CommaExpected,
    ErrorCode::CloseBraceExpected,
    ErrorCode::CloseBracketExpected,
    ErrorCode::EndOfFileExpected,
    ErrorCode::InvalidCommentToken,
    ErrorCode::UnexpectedEndOfComment,
    ErrorCode::UnexpectedEndOfString,
    ErrorCode::UnexpectedEndOfNumber,
    ErrorCode::InvalidUnicode,
    ErrorCode::InvalidEscapeCharacter,
    ErrorCode::InvalidCharacter,
];

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/jsonc.json")).expect("fixture")
}

#[test]
fn parse_matches_typescript_values_errors_and_messages() {
    let fixture = fixture();
    for (name, case) in fixture["parse"].as_object().expect("parse cases") {
        let text = case["text"].as_str().expect("text");
        let (value, errors) = jsonc::parse(text);
        assert_eq!(
            value.unwrap_or(Value::Null),
            case["value"],
            "value for {name}"
        );
        let expected: Vec<(ErrorCode, u64, u64)> = case["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .map(|e| {
                let code = CODES[e["error"].as_u64().expect("code") as usize - 1];
                (
                    code,
                    e["offset"].as_u64().expect("offset"),
                    e["length"].as_u64().expect("length"),
                )
            })
            .collect();
        let actual: Vec<(ErrorCode, u64, u64)> = errors
            .iter()
            .map(|e| (e.error, e.offset as u64, e.length as u64))
            .collect();
        assert_eq!(actual, expected, "errors for {name}");
        let message = jsonc::parse_strict(text).err();
        assert_eq!(
            message.as_deref(),
            case["message"].as_str(),
            "message for {name}"
        );
    }
}

#[test]
fn modify_matches_typescript_comment_preserving_edits() {
    let fixture = fixture();
    for case in fixture["modify"].as_array().expect("modify cases") {
        let name = case["name"].as_str().expect("name");
        let text = case["text"].as_str().expect("text");
        let path: Vec<String> = case["path"]
            .as_array()
            .expect("path")
            .iter()
            .map(|p| p.as_str().expect("seg").to_string())
            .collect();
        let value = (!case["remove"].as_bool().expect("remove")).then(|| case["value"].clone());
        let result = jsonc::modify(text, &path, value.as_ref());
        match case["out"].as_str() {
            Some(out) => assert_eq!(result.as_deref().ok(), Some(out), "modify {name}"),
            None => assert!(result.is_err(), "modify {name} should fail"),
        }
    }
}
