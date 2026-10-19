//! Purpose: exercise actual TUI publications and FIFO queue exchange through Process.
//! Scope: isolated integration acceptance module.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: every observed event comes from the real EventStore listener.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, call, json};
use opencode_event_api::EventStore;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn tui_publication_and_control_queues_preserve_real_payloads() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let output = observed.clone();
    let unsubscribe = process.events.listen(Arc::new(move |event| {
        output.lock().unwrap().push(event.clone());
        Ok(())
    }));
    for (operation, payload) in [
        ("tui.openThemes", None),
        ("tui.executeCommand", Some(json!({"command":"unknown"}))),
        ("tui.executeCommand", Some(json!({"command":"session_new"}))),
        ("tui.appendPrompt", Some(json!({"text":"fixture"}))),
    ] {
        assert_eq!(
            call(
                &process,
                &checkout,
                operation,
                json!({}),
                json!({}),
                payload
            )
            .await
            .unwrap(),
            json!(true)
        );
    }
    let events = observed.lock().unwrap().clone();
    assert_eq!(events[0].data, json!({"command":"session.list"}));
    assert_eq!(events[1].data, json!({}));
    assert_eq!(events[2].data, json!({"command":"session.new"}));
    assert_eq!(events[3].event_type, "tui.prompt.append");
    assert_eq!(events[3].data, json!({"text":"fixture"}));
    assert_eq!(
        events[3].location.as_ref().unwrap().directory,
        checkout.display().to_string()
    );
    process
        .global
        .tui_control()
        .submit_request("/fixture".into(), json!({"value":1}))
        .unwrap();
    assert_eq!(
        call(
            &process,
            &checkout,
            "tui.control.next",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!({"path":"/fixture","body":{"value":1}})
    );
    call(
        &process,
        &checkout,
        "tui.control.response",
        json!({}),
        json!({}),
        Some(json!(["reply"])),
    )
    .await
    .unwrap();
    assert_eq!(
        process.global.tui_control().next_response().await.unwrap(),
        json!(["reply"])
    );
    unsubscribe();
    process.shutdown().await;
}
