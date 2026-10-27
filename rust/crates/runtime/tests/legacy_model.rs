//! Purpose: production V1 provider execution acceptance through public Process operations.
//! Scope: real controlled TCP model, native tools, child composition and abort cleanup.
//! Invariants: tests assert persisted V1 messages/parts, never synthetic V2 projections.
//! Contract tests: this integration target.

#[path = "support/legacy_host.rs"]
mod legacy_host;
#[path = "support/provider.rs"]
mod provider;

use legacy_host::Host;
use provider::{Provider, Reply};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_prompt_streams_native_tool_result_and_restarts() {
    let mut provider = Provider::start(vec![
        Reply::Tool {
            name: "read",
            arguments: json!({"filePath":"evidence.txt"}),
        },
        Reply::Text("legacy native response"),
    ])
    .await;
    let host = Host::start(Some(&provider.url)).await;
    assert!(host.root.path().exists());
    let info = host.create("legacy tool").await;
    let id = info["id"].as_str().unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(15),
        host.call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"Read evidence.txt"}]})),
        ),
    )
    .await
    .unwrap();
    assert_eq!(reply["info"]["role"], "assistant");
    assert_eq!(reply["info"]["finish"], "stop");
    assert!(reply["info"]["time"]["completed"].is_number());
    assert!(
        reply["parts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|part| part["type"] == "text" && part["text"] == "legacy native response")
    );
    let history = host.messages(id).await;
    let tool = history
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().unwrap())
        .find(|part| part["type"] == "tool")
        .unwrap();
    assert_eq!(tool["state"]["status"], "completed");
    assert!(
        tool["state"]["output"]
            .as_str()
            .unwrap()
            .contains("native legacy read evidence"),
        "{tool}"
    );
    let first = provider.requests.recv().await.unwrap().1;
    assert_eq!(first["model"], "local-test");
    let second = provider.requests.recv().await.unwrap().1;
    assert!(
        second["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("native legacy read evidence")))
    );
    let location = opencode_session_api::LocationRef {
        directory: opencode_schema::values::AbsolutePath::new(host.directory.display().to_string()),
        workspace_id: None,
    };
    host.process.locations.invalidate(&location).await;
    host.process.locations.wait_teardowns().await;
    assert!(
        !host.process.locations.contains(&location),
        "settled V1 turn retained its Location lease"
    );
    let options = host.options.clone();
    let directory = host.directory.clone();
    let id = id.to_owned();
    host.process.shutdown().await;
    let process = opencode_runtime::Process::start(options).await.unwrap();
    let messages = process
        .legacy_command(
            "session.messages",
            directory,
            serde_json::from_value(json!({"sessionID":id})).unwrap(),
            Default::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(messages, history);
    process.shutdown().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_provider_rejection_persists_failure_and_releases_runner() {
    let mut provider = Provider::start(vec![Reply::Failure]).await;
    let host = Host::start(Some(&provider.url)).await;
    let session = host.create("provider rejection").await;
    let id = session["id"].as_str().unwrap();
    let result = host
        .process
        .legacy_command(
            "session.prompt",
            host.directory.clone(),
            serde_json::from_value(json!({"sessionID":id})).unwrap(),
            Default::default(),
            Some(json!({"parts":[{"type":"text","text":"Reject this turn"}]})),
        )
        .await;
    let result = result.unwrap();
    assert!(result["info"]["error"].is_object(), "{result}");
    let _ = provider.requests.recv().await.unwrap();
    let history = host.messages(id).await;
    let assistant = history
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["info"]["role"] == "assistant")
        .unwrap();
    assert!(
        assistant["info"]["time"]["completed"].is_number(),
        "{assistant}"
    );
    assert!(
        assistant["info"]["error"]["data"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("controlled rejection")),
        "{assistant}"
    );
    assert_eq!(
        host.call("session.status", None, json!({}), None).await,
        json!({})
    );
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_joins_actual_async_legacy_transport() {
    let mut provider = Provider::start(vec![Reply::Hold]).await;
    let host = Host::start(Some(&provider.url)).await;
    let session = host.create("shutdown legacy").await;
    let id = session["id"].as_str().unwrap();
    host.call(
        "session.prompt_async",
        Some(id),
        json!({}),
        Some(json!({"parts":[{"type":"text","text":"Wait for shutdown"}]})),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), host.process.shutdown())
        .await
        .unwrap();
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_abort_joins_transport_and_persists_terminal_assistant() {
    let mut provider = Provider::start(vec![Reply::Hold]).await;
    let host = Host::start(Some(&provider.url)).await;
    let info = host.create("abort legacy").await;
    let id = info["id"].as_str().unwrap();
    host.call(
        "session.prompt_async",
        Some(id),
        json!({}),
        Some(json!({"parts":[{"type":"text","text":"wait"}]})),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    host.call("session.abort", Some(id), json!({}), None).await;
    let history = host.messages(id).await;
    let assistant = history
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["info"]["role"] == "assistant")
        .unwrap();
    assert!(
        assistant["info"]["time"]["completed"].is_number(),
        "{assistant}"
    );
    assert_eq!(
        host.call("session.status", None, json!({}), None).await,
        json!({})
    );
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_task_runs_child_on_same_legacy_service_and_parent_continues() {
    let mut provider=Provider::start(vec![Reply::Tool{name:"task",arguments:json!({"description":"inspect","prompt":"Answer as child","subagent_type":"general"})},Reply::Text("child evidence"),Reply::Text("parent evidence")]).await;
    let host = Host::start(Some(&provider.url)).await;
    let info = host.create("task parent").await;
    let id = info["id"].as_str().unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(20),
        host.call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"Delegate inspection"}]})),
        ),
    )
    .await
    .unwrap();
    assert!(
        reply["parts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|part| part["text"] == "parent evidence"),
        "{reply}"
    );
    let children = host
        .call("session.children", Some(id), json!({}), None)
        .await;
    assert_eq!(children.as_array().unwrap().len(), 1, "{children}");
    let child = children[0]["id"].as_str().unwrap();
    assert!(
        host.messages(child)
            .await
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| message["parts"].as_array().unwrap())
            .any(|part| part["text"] == "child evidence")
    );
    let _ = provider.requests.recv().await.unwrap();
    let _ = provider.requests.recv().await.unwrap();
    let final_request = provider.requests.recv().await.unwrap().1;
    assert!(
        final_request["messages"]
            .to_string()
            .contains("child evidence")
    );
    host.stop().await;
    provider.stop().await;
}
