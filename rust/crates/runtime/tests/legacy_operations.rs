//! Purpose: real persisted legacy lifecycle and cursor behavior through public Process operations.
//! Scope: admission-only prompt, metadata, import, fork, recursive deletion and page boundaries.
//! Invariants: source rows remain V1 rows; imported IDs and message references are preserved.
//! Contract tests: this integration target.

#[path = "support/legacy_host.rs"]
mod legacy_host;
#[allow(dead_code)]
#[path = "support/provider.rs"]
mod provider;

use legacy_host::Host;
use opencode_session_runtime::legacy::pagination::MessageCursor;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_legacy_lifecycle_import_and_cursor_use_real_rows() {
    let provider = provider::Provider::start(vec![]).await;
    let host = Host::start(Some(&provider.url)).await;
    assert!(host.root.path().exists());
    assert!(host.options.paths.config.exists());
    let original = host.create("Original").await;
    let id = original["id"].as_str().unwrap();
    let first = host
        .call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"first persisted input"}],"noReply":true})),
        )
        .await;
    let second = host
        .call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"second persisted input"}],"noReply":true})),
        )
        .await;
    let third = host
        .call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"third persisted input"}],"noReply":true})),
        )
        .await;
    let history = host.messages(id).await;
    assert_eq!(history.as_array().unwrap().len(), 3);
    let newest = host
        .call("session.messages", Some(id), json!({"limit":2}), None)
        .await;
    assert_eq!(newest, json!([second.clone(), third]));
    let page = host
        .process
        .services()
        .legacy_sessions
        .unwrap()
        .handle_response(
            "session.messages",
            &opencode_server::middleware::legacy_routing::RouteContext {
                directory: host.directory.display().to_string(),
                workspace_id: None,
            },
            &serde_json::from_value(json!({"sessionID":id})).unwrap(),
            &serde_json::from_value(json!({"limit":2})).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.body, newest);
    let cursor = page
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-next-cursor"))
        .unwrap()
        .1
        .clone();
    assert_eq!(
        MessageCursor::decode(&cursor).unwrap().id,
        second["info"]["id"].as_str().unwrap()
    );
    assert_eq!(
        host.call(
            "session.messages",
            Some(id),
            json!({"limit":2,"before":cursor}),
            None
        )
        .await,
        json!([first.clone()])
    );
    let renamed = host.call("session.update",Some(id),json!({}),Some(json!({"title":"Renamed","metadata":{"proof":"stored"},"permission":[{"permission":"read","pattern":"*","action":"allow"}]}))).await;
    assert_eq!(renamed["title"], "Renamed");
    assert_eq!(renamed["metadata"]["proof"], "stored");
    let fork = host
        .call(
            "session.fork",
            Some(id),
            json!({}),
            Some(json!({"messageID":second["info"]["id"]})),
        )
        .await;
    let fork_id = fork["id"].as_str().unwrap();
    assert_eq!(fork["title"], "Renamed (fork #1)");
    let fork_history = host.messages(fork_id).await;
    assert_eq!(fork_history.as_array().unwrap().len(), 1);
    assert_ne!(fork_history[0]["info"]["id"], first["info"]["id"]);
    assert_eq!(fork_history[0]["parts"][0]["text"], "first persisted input");
    assert_eq!(host.messages(id).await, history);
    let child = host
        .call(
            "session.create",
            None,
            json!({}),
            Some(json!({"parentID":id,"title":"Child"})),
        )
        .await;
    assert_eq!(
        host.call("session.children", Some(id), json!({}), None)
            .await[0]["id"],
        child["id"]
    );
    let export = json!({"info":renamed,"messages":history});
    assert_eq!(
        host.call("session.delete", Some(id), json!({}), None).await,
        Value::Bool(true)
    );
    let imported = host
        .call("session.import", None, json!({}), Some(export.clone()))
        .await;
    assert_eq!(imported["id"], id);
    assert_eq!(host.messages(id).await, export["messages"]);
    host.call("session.import", None, json!({}), Some(export))
        .await;
    assert_eq!(host.messages(id).await.as_array().unwrap().len(), 3);
    host.stop().await;
    provider.stop().await;
}
