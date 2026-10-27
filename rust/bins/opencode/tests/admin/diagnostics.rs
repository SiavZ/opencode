//! Purpose: executable CLI debug redaction and V2 fork regressions.
//! Scope: disposable HOME, actual process services and controlled provider.
//! Uses:
//! rust/bins/
//! `-- opencode/tests/support/mod.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- opencode/tests/commands.rs
//! Invariants: no live credentials or user sessions are touched.
//! Contract tests: self

use super::super::support::{Home, Provider, assert_success, output};
use serde_json::{Value, json};

#[test]
fn config_redaction_uses_resolved_precedence_and_does_not_change_provider_requests() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    let path = home.0.path().join("config/opencode/opencode.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["provider"]["local"]["options"]["headers"] = json!({"X-Test":"private-header"});
    std::fs::write(path, config.to_string()).unwrap();
    std::fs::write(
        home.0.path().join("project/opencode.json"),
        json!({"username":"project precedence"}).to_string(),
    )
    .unwrap();
    let result = output(home.command(&["debug", "config"]), None);
    assert_success(&result);
    let config: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(config["username"], "project precedence");
    assert_eq!(config["provider"]["local"]["options"]["apiKey"], "***");
    assert_eq!(
        config["provider"]["local"]["options"]["headers"]["X-Test"],
        "***"
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("local-test-only"));
    let result = output(home.command(&["run", "after debug"]), None);
    assert_success(&result);
    let (headers, _) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer local-test-only")
    );
    assert!(headers.contains("private-header"));
}

#[test]
fn run_fork_replays_visible_history_without_modifying_source() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    assert_success(&output(home.command(&["run", "original prompt"]), None));
    provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let listing = output(home.command(&["session", "list", "--format", "json"]), None);
    assert_success(&listing);
    let sessions: Value = serde_json::from_slice(&listing.stdout).unwrap();
    let source = sessions[0]["id"].as_str().unwrap();
    let result = output(
        home.command(&["run", "fork prompt", "--session", source, "--fork"]),
        None,
    );
    assert_success(&result);
    let (_, body) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body["messages"].to_string().contains("original prompt"));
    assert!(body["messages"].to_string().contains("fork prompt"));
    let result = output(
        home.command(&["run", "source again", "--session", source]),
        None,
    );
    assert_success(&result);
    let (_, body) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body["messages"].to_string().contains("original prompt"));
    assert!(!body["messages"].to_string().contains("fork prompt"));
    let result = output(home.command(&["run", "bad fork", "--fork"]), None);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("--fork requires --continue or --session")
    );
    assert!(provider.requests.try_recv().is_err());
}
