//! Purpose: prove legacy workspace warp against actual native worktree and Session domains.
//! Scope: isolated SQLite process, temporary Git repository and local workspace target.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: workspace creation and transfer use production public service operations.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, call, json};
use opencode_schema::location::Ref;
use opencode_schema::values::AbsolutePath;
use opencode_session_api::operations::{CreateRequest, SessionService};

#[tokio::test]
async fn legacy_workspace_warp_updates_actual_session_and_can_return_local() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().canonicalize().unwrap().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .vars
        .insert("OPENCODE_EXPERIMENTAL_WORKSPACES".into(), "true".into());
    options.git_env = vec![
        (
            "GIT_CONFIG_GLOBAL".into(),
            root.path().join("empty").display().to_string(),
        ),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ];
    let process = Process::start(options).await.unwrap();
    call(
        &process,
        &checkout,
        "project.initGit",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    let output = std::process::Command::new("git")
        .current_dir(&checkout)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ])
        .env("GIT_CONFIG_GLOBAL", root.path().join("empty"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let space = call(
        &process,
        &checkout,
        "experimental.workspace.create",
        json!({}),
        json!({}),
        Some(json!({"type":"worktree"})),
    )
    .await
    .unwrap();
    let session = process
        .session
        .create(CreateRequest {
            id: None,
            agent: None,
            model: None,
            location: Ref {
                directory: AbsolutePath::new(checkout.display().to_string()),
                workspace_id: None,
            },
        })
        .await
        .unwrap();
    call(
        &process,
        &checkout,
        "experimental.workspace.warp",
        json!({}),
        json!({}),
        Some(json!({"id":space["id"],"sessionID":session.id,"copyChanges":false})),
    )
    .await
    .unwrap();
    assert_eq!(
        process
            .session
            .get(&session.id)
            .await
            .unwrap()
            .location
            .workspace_id
            .unwrap()
            .as_str(),
        space["id"].as_str().unwrap()
    );
    call(
        &process,
        &checkout,
        "experimental.workspace.warp",
        json!({}),
        json!({}),
        Some(json!({"id":null,"sessionID":session.id,"copyChanges":false})),
    )
    .await
    .unwrap();
    assert!(
        process
            .session
            .get(&session.id)
            .await
            .unwrap()
            .location
            .workspace_id
            .is_none()
    );
    process.shutdown().await;
}
