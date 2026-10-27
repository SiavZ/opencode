//! Purpose: executable global credential listing/removal with restart and secret redaction.
//! Scope: disposable SQLite credentials and read-only models.dev fixture.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: no real credentials, project bootstrap or public catalog traffic.
//! Contract tests: self

use super::super::support::{Home, assert_success, output};

#[test]
fn global_provider_list_and_explicit_logout_persist_without_project_bootstrap() {
    let home = Home::new();
    let seeded = output(home.command(&["db", "INSERT INTO credential (id,integration_id,label,value,time_created,time_updated) VALUES ('fixture-id','fixture','default','{\"type\":\"key\",\"key\":\"private-key-never-print\"}',1,1)"]), None);
    assert_success(&seeded);
    let catalog = home.0.path().join("catalog.json");
    std::fs::write(&catalog, r#"{"fixture":{"id":"fixture","name":"Fixture provider","env":["FIXTURE_KEY"],"models":{}}}"#).unwrap();
    std::fs::write(
        home.0.path().join("project/opencode.json"),
        "invalid project json",
    )
    .unwrap();
    std::fs::write(
        home.0.path().join("config/opencode/opencode.json"),
        "invalid global json",
    )
    .unwrap();
    let mut command = home.command(&["auth", "list"]);
    command
        .env("OPENCODE_MODELS_PATH", &catalog)
        .env("FIXTURE_KEY", "private-env-never-print");
    let listed = output(command, None);
    assert_success(&listed);
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("Fixture provider api"), "{text}");
    assert!(text.contains("1 credentials"));
    assert!(text.contains("Fixture provider FIXTURE_KEY"));
    assert!(text.contains("1 environment variable"));
    assert!(!text.contains("private-"));
    let mut command = home.command(&["providers", "logout", "unknown"]);
    command.env("OPENCODE_MODELS_PATH", &catalog);
    let missing = output(command, None);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Unknown configured provider"));
    let mut command = home.command(&["auth", "logout", "FIXTURE PROVIDER"]);
    command.env("OPENCODE_MODELS_PATH", &catalog);
    let removed = output(command, None);
    assert_success(&removed);
    assert!(String::from_utf8_lossy(&removed.stdout).contains("Logout successful"));
    let mut command = home.command(&["auth", "list"]);
    command.env("OPENCODE_MODELS_PATH", &catalog);
    let restarted = output(command, None);
    assert_success(&restarted);
    assert!(String::from_utf8_lossy(&restarted.stdout).contains("0 credentials"));
    assert!(!String::from_utf8_lossy(&restarted.stdout).contains("Fixture provider api"));
}
