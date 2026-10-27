//! Purpose: validate actual hosted worker process startup and stdin command dispatch.
//! Scope: native subprocess fixtures without reachable database or external transports.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/workers.rs
//! `-- hosted/src/main.rs
//! External:
//! |-- serde_json::{Value,json}
//! `-- std::process::{Command,Stdio}
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests use unreachable loopback database; empty/stat/log/inventory must not touch it.
//! Contract tests: this file.

use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn execute(command: &str, input: &Value) -> std::process::Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_opencode-hosted"))
        .env("OPENCODE_HOSTED_SERVICE", "console-worker")
        .env("OPENCODE_CONSOLE_WORKER_COMMAND", command)
        .env(
            "OPENCODE_CONSOLE_DATABASE_URL",
            "mysql://root@127.0.0.1:1/p12_cli",
        )
        .env_remove("OPENCODE_CONSOLE_OPERATOR_CONFIRM_DATABASE")
        .env_remove("OPENCODE_CONSOLE_WORKER_CAPTURE_FILE")
        .env_remove("OPENCODE_CONSOLE_WORKER_CAPTURE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    process.wait_with_output().unwrap()
}

#[test]
fn console_workers_log_process_validates_ingress_before_database_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-hosted"))
        .env("OPENCODE_HOSTED_SERVICE", "console-log-processor")
        .env_remove("OPENCODE_CONSOLE_DATABASE_URL")
        .env("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN", "")
        .env_remove("OPENCODE_CONSOLE_WORKER_CAPTURE_FILE")
        .env_remove("OPENCODE_CONSOLE_WORKER_CAPTURE_URL")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN"),
        "{error}"
    );
    assert!(!error.contains("OPENCODE_CONSOLE_DATABASE_URL"), "{error}");
}

#[test]
fn console_workers_process_modes_start_without_console_auth_configuration() {
    for (command, input, expected) in [
        ("stat", json!({"ids":[]}), json!({})),
        (
            "log-processor",
            json!([]),
            json!({"console":[],"events":[]}),
        ),
    ] {
        let output = execute(command, &input);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected
        );
    }
    let output = execute("inventory", &json!({}));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inventory: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inventory["native"].as_array().unwrap().len(), 18);
    let blocked = execute(
        "create-coupon",
        &json!({"email":"blocked@example.test","type":"BUILDATHON"}),
    );
    assert!(!blocked.status.success());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("confirmation")
            || String::from_utf8_lossy(&blocked.stderr).contains("CONFIRM_DATABASE")
    );
}
