//! Purpose: contract tests for keybind parsing and defaults.
//! Scope: pure values, parsing, and default definitions.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/keybind.rs
//! External:
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: parsing handles leader, modifiers, and comma-separated lists correctly.
//! Contract tests: this file.

use opencode_ui_terminal::keybind::{parse_binding_value, parse_key_stroke, BindingValue, DEFAULT_KEYBINDS};
use std::collections::HashMap;

#[test]
fn test_parse_key_stroke_simple() {
    let ks = parse_key_stroke("a").unwrap();
    assert_eq!(ks.name, "a");
    assert!(!ks.ctrl);
    assert!(!ks.shift);
}

#[test]
fn test_parse_key_stroke_modifiers() {
    let ks = parse_key_stroke("ctrl+shift+a").unwrap();
    assert_eq!(ks.name, "a");
    assert!(ks.ctrl);
    assert!(ks.shift);
    assert!(!ks.meta);
}

#[test]
fn test_parse_key_stroke_leader() {
    let ks = parse_key_stroke("<leader>n").unwrap();
    assert_eq!(ks.name, "<leader>n");
    assert!(!ks.ctrl);
}

#[test]
fn test_parse_key_stroke_multiple_modifiers() {
    let ks = parse_key_stroke("ctrl+alt+b").unwrap();
    assert_eq!(ks.name, "b");
    assert!(ks.ctrl);
    assert!(ks.meta); // alt maps to meta
}

#[test]
fn test_parse_binding_value_none() {
    let bv = parse_binding_value("none").unwrap();
    assert_eq!(bv, BindingValue::None);
}

#[test]
fn test_parse_binding_value_disabled() {
    let bv = parse_binding_value("false").unwrap();
    assert_eq!(bv, BindingValue::Disabled);
}

#[test]
fn test_parse_binding_value_single() {
    let bv = parse_binding_value("ctrl+x").unwrap();
    if let BindingValue::Single(ks) = bv {
        assert_eq!(ks.name, "x");
        assert!(ks.ctrl);
    } else {
        panic!("Expected Single");
    }
}

#[test]
fn test_parse_binding_value_multiple() {
    let bv = parse_binding_value("ctrl+c,ctrl+d").unwrap();
    if let BindingValue::Multiple(strokes) = bv {
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[0].name, "c");
        assert!(strokes[0].ctrl);
        assert_eq!(strokes[1].name, "d");
        assert!(strokes[1].ctrl);
    } else {
        panic!("Expected Multiple");
    }
}

#[test]
fn test_defaults_contain_leader() {
    let leader = DEFAULT_KEYBINDS.get("leader").unwrap();
    if let BindingValue::Single(ks) = leader {
        assert_eq!(ks.name, "x");
        assert!(ks.ctrl);
    } else {
        panic!("Expected Single for leader");
    }
}

#[test]
fn test_defaults_contain_session_new() {
    let session_new = DEFAULT_KEYBINDS.get("session_new").unwrap();
    if let BindingValue::Single(ks) = session_new {
        assert_eq!(ks.name, "<leader>n");
    } else {
        panic!("Expected Single for session_new");
    }
}

#[test]
fn test_defaults_contain_app_exit() {
    let app_exit = DEFAULT_KEYBINDS.get("app_exit").unwrap();
    if let BindingValue::Multiple(strokes) = app_exit {
        assert_eq!(strokes.len(), 3);
        assert_eq!(strokes[0].name, "c");
        assert_eq!(strokes[1].name, "d");
        assert_eq!(strokes[2].name, "<leader>q");
    } else {
        panic!("Expected Multiple for app_exit");
    }
}

#[test]
fn test_parse_overrides_valid() {
    let mut overrides = HashMap::new();
    overrides.insert("leader".to_string(), serde_json::json!("ctrl+y"));
    let parsed = opencode_ui_terminal::keybind::parse_overrides(&overrides).unwrap();
    assert_eq!(parsed.len(), 1);

    let merged = opencode_ui_terminal::keybind::merge_defaults(&parsed);
    let leader = merged.get("leader").unwrap();
    if let BindingValue::Single(ks) = leader {
        assert_eq!(ks.name, "y");
        assert!(ks.ctrl);
    } else {
        panic!("Expected Single for merged leader");
    }

    // Check that other defaults are preserved
    let session_new = merged.get("session_new").unwrap();
    if let BindingValue::Single(ks) = session_new {
        assert_eq!(ks.name, "<leader>n");
    } else {
        panic!("Expected Single for session_new");
    }
}

#[test]
fn test_parse_overrides_invalid() {
    let mut overrides = HashMap::new();
    overrides.insert("invalid_key".to_string(), serde_json::json!("ctrl+z"));
    let result = opencode_ui_terminal::keybind::parse_overrides(&overrides);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unrecognized keybind"));
}
