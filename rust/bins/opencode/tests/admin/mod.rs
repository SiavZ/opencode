//! Purpose: executable regressions for CLI administration and implicit model selection.
//! Scope: isolated files, real persistence and controlled local provider only.
//! Uses:
//! rust/bins/
//! `-- opencode/tests/support/mod.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- opencode/tests/commands.rs
//! Invariants: every mutation is confined to a disposable HOME and project directory.
//! Contract tests: self

use super::support::{Home, Provider, assert_success, output};
use serde_json::{Value, json};
mod agents;
mod config;
mod database;
mod diagnostics;
mod integrations;
mod json_run;
mod models;
#[cfg(unix)]
mod permissions;
mod providers;

#[test]
fn paths_follow_isolated_xdg_and_test_home_without_opening_database() {
    let home = Home::new();
    let result = output(home.command(&["debug", "paths"]), None);
    assert_success(&result);
    let stdout = String::from_utf8(result.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 9);
    for (index, (key, suffix)) in [
        ("home", "home"),
        ("data", "data/opencode"),
        ("bin", "cache/opencode/bin"),
        ("log", "data/opencode/log"),
        ("repos", "data/opencode/repos"),
        ("cache", "cache/opencode"),
        ("config", "config/opencode"),
        ("state", "state/opencode"),
        ("tmp", "tmp/opencode"),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            lines[index],
            format!("{key:10} {}", home.0.path().join(suffix).display())
        );
    }
    assert!(!home.0.path().join("data/opencode/opencode.db").exists());
}

#[test]
fn implicit_variant_uses_default_model_and_unknown_variant_fails_before_provider() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config_v2(&provider.url);
    let path = home.0.path().join("config/opencode/opencode.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config.as_object_mut().unwrap().remove("model");
    config["providers"]["local"]["models"]["local-test"]["variants"] =
        json!([{"id":"high","headers":{},"body":{"reasoning_effort":"high"}}]);
    std::fs::write(path, config.to_string()).unwrap();
    let result = output(
        home.command(&["run", "variant prompt", "--variant", "high"]),
        None,
    );
    assert_success(&result);
    let (_, body) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(body["model"], "local-test");
    assert_eq!(body["reasoning_effort"], "high");
    let result = output(
        home.command(&["run", "variant prompt", "--variant", "absent"]),
        None,
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("Variant unavailable"));
    assert!(provider.requests.try_recv().is_err());
}

#[test]
fn import_export_restart_sanitize_and_delete_use_durable_legacy_services() {
    let home = Home::new();
    let file = home.0.path().join("project/transcript.json");
    let data = json!({
        "info":{"id":"ses_cli_import", "slug":"fixture", "projectID":"global", "directory":"/old", "title":"private title", "version":"local", "time":{"created":1,"updated":2}},
        "messages":[{"info":{"id":"msg_cli_import", "sessionID":"ses_cli_import", "role":"user", "time":{"created":1}, "agent":"build", "model":{"providerID":"local","modelID":"local-test"}},
        "parts":[{"id":"prt_cli_import", "sessionID":"ses_cli_import", "messageID":"msg_cli_import", "type":"text", "text":"private transcript", "time":{"start":1,"end":2}}]}]
    });
    std::fs::write(&file, data.to_string()).unwrap();
    let imported = output(home.command(&["import", "transcript.json"]), None);
    assert_success(&imported);
    assert_eq!(
        String::from_utf8_lossy(&imported.stdout),
        "Imported session: ses_cli_import\n"
    );
    let listed = output(home.command(&["session", "list", "--format", "json"]), None);
    assert_success(&listed);
    let list: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(list[0]["id"], "ses_cli_import");
    assert_eq!(list[0]["projectId"], "global");
    assert_eq!(
        list[0]["directory"],
        home.0
            .path()
            .join("project")
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    let exported = output(home.command(&["export", "ses_cli_import"]), None);
    assert_success(&exported);
    let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(exported["info"]["id"], data["info"]["id"]);
    assert_eq!(
        exported["messages"][0]["info"]["id"],
        data["messages"][0]["info"]["id"]
    );
    assert_eq!(
        exported["messages"][0]["info"]["sessionID"],
        data["info"]["id"]
    );
    assert_eq!(
        exported["messages"][0]["parts"][0]["id"],
        data["messages"][0]["parts"][0]["id"]
    );
    assert_eq!(
        exported["messages"][0]["parts"][0]["messageID"],
        data["messages"][0]["info"]["id"]
    );
    assert_eq!(
        exported["messages"][0]["parts"][0]["text"],
        "private transcript"
    );
    let sanitized = output(
        home.command(&["export", "ses_cli_import", "--sanitize"]),
        None,
    );
    assert_success(&sanitized);
    let sanitized: Value = serde_json::from_slice(&sanitized.stdout).unwrap();
    assert_eq!(sanitized["info"]["id"], exported["info"]["id"]);
    assert_eq!(
        sanitized["messages"][0]["info"]["id"],
        exported["messages"][0]["info"]["id"]
    );
    assert_eq!(
        sanitized["messages"][0]["parts"][0]["id"],
        exported["messages"][0]["parts"][0]["id"]
    );
    assert_eq!(
        sanitized["messages"][0]["parts"][0]["messageID"],
        exported["messages"][0]["parts"][0]["messageID"]
    );
    assert_eq!(
        sanitized["info"]["title"],
        "[redacted:session-title:ses_cli_import]"
    );
    assert_eq!(
        sanitized["messages"][0]["parts"][0]["text"],
        "[redacted:text:prt_cli_import]"
    );
    assert_eq!(sanitized["info"]["time"], exported["info"]["time"]);
    let deleted = output(home.command(&["session", "delete", "ses_cli_import"]), None);
    assert_success(&deleted);
    let missing = output(home.command(&["export", "ses_cli_import"]), None);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Session not found: ses_cli_import"));
}

#[test]
fn import_errors_never_create_a_session() {
    let home = Home::new();
    std::fs::write(home.0.path().join("project/bad.json"), "{").unwrap();
    for (file, expected) in [
        ("missing.json", "File not found"),
        ("bad.json", "Invalid JSON"),
    ] {
        let result = output(home.command(&["import", file]), None);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
    }
    let listed = output(home.command(&["session", "list", "--format", "json"]), None);
    assert_success(&listed);
    assert!(listed.stdout.is_empty());
}
