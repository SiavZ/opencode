//! Purpose: prove V2 relocation changes persisted Session location through durable events.
//! Scope: isolated Process and actual SessionService admission.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: no direct SQL fixtures or copied relocation logic.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, call, json};
use opencode_schema::location::Ref;
use opencode_schema::values::AbsolutePath;
use opencode_session_api::operations::{CreateRequest, SessionService};

#[tokio::test]
async fn move_session_publishes_durable_event_and_changes_projected_location() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    let session = process
        .session
        .create(CreateRequest {
            id: None,
            agent: None,
            model: None,
            location: Ref {
                directory: AbsolutePath::new(source.display().to_string()),
                workspace_id: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(
        call(
            &process,
            &source,
            "experimental.controlPlane.moveSession",
            json!({}),
            json!({}),
            Some(json!({"sessionID":session.id,"destination":{"directory":destination}}))
        )
        .await
        .unwrap(),
        serde_json::Value::Null
    );
    let moved = process.session.get(&session.id).await.unwrap();
    assert_eq!(
        moved.location.directory.as_str(),
        destination.display().to_string()
    );
    let history = call(
        &process,
        &destination,
        "sync.history.list",
        json!({}),
        json!({}),
        Some(json!({})),
    )
    .await
    .unwrap();
    let history = history.as_array().unwrap();
    assert!(
        history
            .iter()
            .any(|event| event["type"] == "session.next.moved.1"
                && event["aggregate_id"] == session.id.as_str())
    );
    let count = history.len();
    call(
        &process,
        &destination,
        "experimental.controlPlane.moveSession",
        json!({}),
        json!({}),
        Some(json!({"sessionID":session.id,"destination":{"directory":destination}})),
    )
    .await
    .unwrap();
    assert_eq!(
        call(
            &process,
            &destination,
            "sync.history.list",
            json!({}),
            json!({}),
            Some(json!({}))
        )
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .len(),
        count
    );
    process.shutdown().await;
}
