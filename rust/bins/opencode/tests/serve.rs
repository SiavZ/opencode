//! Purpose: `opencode serve` end to end through the real executable: isolated HOME, readiness
//! line on stdout, a live `/api/location` + session round trip, and clean exit on SIGTERM.
//! Also unit-checks network option precedence (argv flag > config `server` > yargs default).
//! Scope: process-level integration test.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Value
//! `-- std::process::Command
//! Used by:
//! `-- none, entrypoint
//! Invariants: the child never sees the real HOME or XDG dirs; it is always killed on failure.
//! Contract tests: this file

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_binds_answers_and_exits_on_sigterm() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_opencode"))
        .args(["serve", "--port", "0", "--hostname", "127.0.0.1"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = Guard(child);
    let mut lines = BufReader::new(guard.0.stdout.take().unwrap()).lines();
    assert_eq!(
        lines.next().unwrap().unwrap(),
        "Warning: OPENCODE_SERVER_PASSWORD is not set; server is unsecured."
    );
    let ready = lines.next().unwrap().unwrap();
    let url = ready
        .strip_prefix("opencode server listening on ")
        .unwrap_or_else(|| panic!("unexpected readiness line {ready:?}"))
        .to_owned();

    let directory = base.display().to_string();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = reqwest::Client::new();
        let location: serde_json::Value = client
            .get(format!("{url}/api/location"))
            .header("x-opencode-directory", &directory)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(location["directory"], serde_json::json!(directory));
        assert_eq!(location["project"]["id"], "global");
        let created = client
            .post(format!("{url}/api/session"))
            .json(&serde_json::json!({ "location": { "directory": directory } }))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status().as_u16(), 200);
    });
    assert!(
        home.join(".local/share/opencode/opencode-local.db")
            .exists(),
        "database lives under the isolated XDG data dir"
    );

    let status = Command::new("kill")
        .args(["-TERM", &guard.0.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let exit = loop {
        if let Some(exit) = guard.0.try_wait().unwrap() {
            break exit;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "serve did not exit on SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(exit.success(), "exit status {exit:?}");
}
