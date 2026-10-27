//! Purpose: prove headless run settles real permission requests by rejection.
//! Scope: disposable files and controlled two-turn tool provider.
//! Uses: opencode/tests/support/mod.rs, opencode/tests/terminal_app/provider.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: a pending ask never hangs or becomes implicit approval.
//! Contract tests: self

use super::super::terminal_app::provider::ToolProvider;

use super::super::support::{Home, assert_success, output};
use serde_json::{Value, json};

#[test]
fn headless_run_rejects_actual_permission_ask_and_continues_without_file_contents() {
    let home = Home::new();
    let provider = ToolProvider::start();
    home.config_v2(&provider.url);
    std::fs::write(
        home.0.path().join("project/evidence.txt"),
        "PRIVATE-FILE-CONTENTS-DO-NOT-READ",
    )
    .unwrap();
    let config = home.0.path().join("config/opencode/opencode.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    value["permissions"] = json!([{"action":"read","resource":"*","effect":"ask"}]);
    std::fs::write(config, value.to_string()).unwrap();
    let result = output(home.command(&["run", "read evidence"]), None);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stderr).contains("auto-rejecting"));
    assert!(String::from_utf8_lossy(&result.stdout).contains("permission continuation"));
    provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let continued = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        !continued
            .to_string()
            .contains("PRIVATE-FILE-CONTENTS-DO-NOT-READ")
    );
    assert!(
        continued["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool")
    );
    assert!(!provider.requests.try_iter().any(|request| {
        request
            .to_string()
            .contains("PRIVATE-FILE-CONTENTS-DO-NOT-READ")
    }));
}
