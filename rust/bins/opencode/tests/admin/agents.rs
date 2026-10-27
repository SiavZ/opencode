//! Purpose: executable legacy agent ordering and permission overlay acceptance.
//! Scope: isolated materialized config, no provider calls.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: native metadata and configured deny rules originate in actual runtime services.
//! Contract tests: self

use super::super::support::{Home, assert_success, output};
use serde_json::json;

#[test]
fn agent_list_materializes_native_agents_then_custom_permission_overlays() {
    let home = Home::new();
    std::fs::write(
        home.0.path().join("config/opencode/opencode.json"),
        json!({"agent":{
            "fixture":{"mode":"subagent","permission":{"bash":"deny"}},
            "plan":{"disable":true}
        }})
        .to_string(),
    )
    .unwrap();
    let listed = output(home.command(&["agent", "list"]), None);
    assert_success(&listed);
    let text = String::from_utf8(listed.stdout).unwrap();
    assert!(text.contains("build (primary)"));
    assert!(!text.contains("plan (primary)"));
    assert!(text.contains("fixture (subagent)"));
    assert!(text.find("title (primary)").unwrap() < text.find("fixture (subagent)").unwrap());
    let fixture = text.split("fixture (subagent)\n").nth(1).unwrap();
    let rules: serde_json::Value = serde_json::from_str(fixture.trim()).unwrap();
    assert!(
        rules
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["permission"] == "bash" && rule["action"] == "deny")
    );
}
