//! Purpose: executable inspection of real MCP state and filesystem skills.
//! Scope: isolated HOME with disabled local transports only.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: no live OAuth, installed programs or public servers are contacted.
//! Contract tests: self

use super::super::support::{Home, assert_success, output};
use serde_json::{Value, json};

#[test]
fn mcp_logout_removes_saved_unconfigured_credentials_and_restart_observes_removal() {
    let home = Home::new();
    let path = home.0.path().join("data/opencode/mcp-auth.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, json!({"saved-only":{"tokens":{"accessToken":"private-token-never-print"},"clientInfo":{"clientId":"fixture-client"}}}).to_string()).unwrap();
    let missing = output(home.command(&["mcp", "logout", "unknown"]), None);
    assert_success(&missing);
    assert!(String::from_utf8_lossy(&missing.stdout).contains("No credentials found for: unknown"));
    let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(stored.get("saved-only").is_some());
    let removed = output(home.command(&["mcp", "logout", "saved-only"]), None);
    assert_success(&removed);
    assert!(
        String::from_utf8_lossy(&removed.stdout)
            .contains("Removed OAuth credentials for saved-only")
    );
    assert!(!String::from_utf8_lossy(&removed.stdout).contains("private-token"));
    let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, json!({}));
    let restarted = output(home.command(&["mcp", "logout", "saved-only"]), None);
    assert_success(&restarted);
    assert!(String::from_utf8_lossy(&restarted.stdout).contains("No MCP OAuth credentials stored"));
}

#[test]
fn mcp_lists_disabled_transport_and_real_unauthenticated_oauth_state() {
    let home = Home::new();
    let config = home.0.path().join("config/opencode/opencode.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        config,
        json!({"mcp":{
            "disabled-local":{"type":"local","command":["never-execute-fixture"],"enabled":false},
            "disabled-remote":{"type":"remote","url":"http://127.0.0.1:1/mcp","enabled":false}
        }})
        .to_string(),
    )
    .unwrap();
    let listed = output(home.command(&["mcp", "list"]), None);
    assert_success(&listed);
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("○ disabled-local disabled"), "{text}");
    assert!(text.contains("never-execute-fixture"));
    assert!(text.contains("○ disabled-remote disabled"));
    assert!(text.contains("2 server(s)"));
    let auth = output(home.command(&["mcp", "auth", "list"]), None);
    assert_success(&auth);
    let text = String::from_utf8_lossy(&auth.stdout);
    assert!(
        text.contains("✗ disabled-remote not authenticated"),
        "{text}"
    );
    assert!(!text.contains("disabled-local"));
    assert!(text.contains("1 OAuth-capable server(s)"));
}

#[test]
fn debug_skill_reads_project_skill_frontmatter_and_content() {
    let home = Home::new();
    let path = home
        .0
        .path()
        .join("project/.opencode/skills/fixture/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        "---\nname: fixture\ndescription: Controlled local skill\n---\nFixture instructions.\n",
    )
    .unwrap();
    let listed = output(home.command(&["debug", "skill"]), None);
    assert_success(&listed);
    let skills: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(
        skills
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "fixture"
                && skill["description"] == "Controlled local skill"),
        "{skills}"
    );
}
