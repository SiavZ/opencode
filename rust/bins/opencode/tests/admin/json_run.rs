//! Purpose: real legacy JSON event execution, persisted parts and provider errors.
//! Scope: isolated CLI subprocesses and controlled loopback provider.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: emitted parts are the same durable parts returned by export after restart.
//! Contract tests: self

use super::super::support::{Home, Provider, assert_success, output};
use serde_json::Value;

#[cfg(unix)]
#[test]
fn json_run_interrupts_real_inflight_provider_and_restart_can_export_session() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let home = Home::new();
    let provider = Provider::delayed(false, Duration::from_secs(30));
    home.config(&provider.url);
    let mut command = home.command(&["run", "interrupt actual provider", "--format", "json"]);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = super::super::support::ChildGuard(command.spawn().unwrap());
    provider
        .requests
        .recv_timeout(Duration::from_secs(20))
        .unwrap();
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "JSON cancellation did not settle"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(child.0.wait().unwrap().code(), Some(1));
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("Session interrupted"), "{stderr}");
    drop(provider);
    let listing = output(home.command(&["session", "list", "--format", "json"]), None);
    assert_success(&listing);
    let sessions: Value = serde_json::from_slice(&listing.stdout).unwrap();
    let id = sessions[0]["id"].as_str().unwrap();
    let exported = output(home.command(&["export", id]), None);
    assert_success(&exported);
    let data: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(data["info"]["id"], id);
    assert!(
        data["messages"]
            .to_string()
            .contains("interrupt actual provider")
    );
    assert!(
        data["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["info"]["role"] == "assistant"
                && message["info"]["time"]["completed"].is_number()),
        "in-flight provider cancellation must settle before CLI exit: {data}"
    );
}

#[test]
fn plain_attachment_and_title_use_actual_legacy_prompt_with_piped_stdin() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    std::fs::write(
        home.0.path().join("project/plain.txt"),
        "PLAIN-LOCAL-ATTACHMENT",
    )
    .unwrap();
    let result = output(
        home.command(&[
            "run",
            "plain prompt",
            "--file",
            "plain.txt",
            "--title",
            "Plain fixture",
        ]),
        Some("plain stdin"),
    );
    assert_success(&result);
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "controlled assistant reply\n"
    );
    let (_, body) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let text = body["messages"].to_string();
    assert!(text.contains("plain prompt"));
    assert!(text.contains("plain stdin"));
    assert!(text.contains("PLAIN-LOCAL-ATTACHMENT"));
    let listing = output(home.command(&["session", "list", "--format", "json"]), None);
    assert_success(&listing);
    let sessions: Value = serde_json::from_slice(&listing.stdout).unwrap();
    assert_eq!(sessions[0]["title"], "Plain fixture");
}

#[test]
fn json_run_streams_real_parts_and_export_preserves_their_ids() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    std::fs::write(
        home.0.path().join("project/attachment.txt"),
        "controlled attachment contents",
    )
    .unwrap();
    let result = output(
        home.command(&[
            "run",
            "json prompt",
            "--format",
            "json",
            "--title",
            "JSON fixture",
            "--file",
            "attachment.txt",
        ]),
        Some("stdin suffix"),
    );
    assert_success(&result);
    let lines: Vec<Value> = String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!lines.is_empty());
    let text = lines
        .iter()
        .find(|line| line["type"] == "text" && line["part"]["text"] == "controlled assistant reply")
        .unwrap();
    assert!(lines.iter().any(|line| line["type"] == "text"
        && line["part"]["id"] == text["part"]["id"]
        && line["part"]["text"] == ""));
    let session = text["sessionID"].as_str().unwrap();
    assert_eq!(text["part"]["text"], "controlled assistant reply");
    assert!(lines.iter().any(|line| line["type"] == "step_start"));
    assert!(lines.iter().any(|line| line["type"] == "step_finish"));
    let deltas = lines
        .iter()
        .filter(|line| line["type"] == "part_delta" && line["partID"] == text["part"]["id"])
        .filter_map(|line| line["delta"].as_str())
        .collect::<String>();
    assert_eq!(deltas, "controlled assistant reply", "{lines:?}");
    assert!(
        lines
            .iter()
            .all(|line| line["sessionID"] == session && line["timestamp"].as_u64().is_some())
    );
    let (_, body) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(body.to_string().contains("json prompt\\nstdin suffix"));
    assert!(
        body.to_string().contains("controlled attachment contents"),
        "{body}"
    );
    let exported = output(home.command(&["export", session]), None);
    assert_success(&exported);
    let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(exported["info"]["title"], "JSON fixture");
    assert!(
        exported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| message["parts"].as_array().unwrap())
            .any(|part| part["id"] == text["part"]["id"] && part["text"] == text["part"]["text"])
    );
}

#[test]
fn json_run_provider_error_is_not_reported_as_success() {
    let home = Home::new();
    let provider = Provider::start(true);
    home.config(&provider.url);
    let result = output(
        home.command(&["run", "error prompt", "--format", "json"]),
        None,
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("controlled provider failure"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let lines: Vec<Value> = String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        !lines
            .iter()
            .any(|line| line["type"] == "text"
                && line["part"]["text"] == "controlled assistant reply")
    );
}
