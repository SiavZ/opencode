//! Purpose: acceptance coverage for command, agent, workspace and synchronization adapters.
//! Scope: real isolated Process and persisted configuration, without external network.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: exercises production adapter dispatch, not copied implementation logic.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, call, json};

#[tokio::test]
async fn catalogs_workspace_and_history_are_real_process_domains() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .vars
        .insert("OPENCODE_EXPERIMENTAL".into(), "true".into());
    let process = Process::start(options).await.unwrap();
    std::fs::write(checkout.join("opencode.json"),serde_json::to_vec(&json!({"command":{"fixture":{"template":"run $10 $2 $2 $ARGUMENTS","description":"Fixture command"}},"agent":{"fixture":{"description":"Configured agent","model":"fixture/model","permission":{"edit":"deny"}}}})).unwrap()).unwrap();
    let commands = call(
        &process,
        &checkout,
        "command.list",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    let commands = commands.as_array().unwrap();
    assert_eq!(commands[0]["name"], "init");
    assert_eq!(commands[1]["name"], "review");
    let command = commands
        .iter()
        .find(|command| command["name"] == "fixture")
        .unwrap();
    assert_eq!(command["template"], "run $10 $2 $2 $ARGUMENTS");
    assert_eq!(command["hints"], json!(["$10", "$2", "$ARGUMENTS"]));
    let agents = call(
        &process,
        &checkout,
        "app.agents",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    let agents = agents.as_array().unwrap();
    assert_eq!(agents[0]["name"], "build");
    let agent = agents
        .iter()
        .find(|agent| agent["name"] == "fixture")
        .unwrap();
    assert_eq!(
        agent["model"],
        json!({"providerID":"fixture","modelID":"model"})
    );
    assert!(
        agent["permission"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["permission"] == "edit" && rule["action"] == "deny")
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "experimental.capabilities.get",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!({"backgroundSubagents":true})
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "experimental.workspace.list",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!([])
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "experimental.workspace.status",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!([])
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "sync.history.list",
            json!({}),
            json!({}),
            Some(json!({}))
        )
        .await
        .unwrap(),
        json!([])
    );
    let error = call(
        &process,
        &checkout,
        "experimental.controlPlane.moveSession",
        json!({}),
        json!({}),
        Some(json!({"sessionID":"ses_missing","destination":{"directory":checkout}})),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        super::LegacyError::Response { status: 400, .. }
    ));
    process.shutdown().await;
}
