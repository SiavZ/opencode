//! Purpose: production V1 compaction acceptance over real transport and persisted history.
//! Scope: manual summary selection and subsequent prompt continuation, not a synthetic runner.
//! Invariants: the summary turn has no tools and continuation uses the persisted summary.
//! Contract tests: this integration target.

#[path = "support/legacy_host.rs"]
mod legacy_host;
#[allow(dead_code)]
#[path = "support/provider.rs"]
mod provider;

use legacy_host::Host;
use provider::{Provider, Reply};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_summary_uses_history_and_continuation_uses_summary() {
    let mut provider = Provider::start(vec![
        Reply::Text("source response"),
        Reply::Text("persisted summary evidence"),
        Reply::Text("continued after summary"),
    ])
    .await;
    let host = Host::start(Some(&provider.url)).await;
    let session = host.create("compact V1").await;
    let id = session["id"].as_str().unwrap();
    tokio::time::timeout(
        Duration::from_secs(15),
        host.call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"Original user evidence"}]})),
        ),
    )
    .await
    .unwrap();
    let _ = provider.requests.recv().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(15),
        host.call(
            "session.summarize",
            Some(id),
            json!({}),
            Some(json!({"providerID":"local","modelID":"local-test","auto":false})),
        ),
    )
    .await
    .unwrap();
    let summary = provider.requests.recv().await.unwrap().1;
    assert!(
        summary["messages"]
            .to_string()
            .contains("Original user evidence"),
        "{summary}"
    );
    assert!(
        summary["tools"].as_array().is_none_or(Vec::is_empty),
        "{summary}"
    );
    let history = host.messages(id).await;
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["info"]["summary"] == true
                && message["parts"]
                    .to_string()
                    .contains("persisted summary evidence")),
        "{history}"
    );
    tokio::time::timeout(
        Duration::from_secs(15),
        host.call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"Continue now"}]})),
        ),
    )
    .await
    .unwrap();
    let continuation = provider.requests.recv().await.unwrap().1;
    assert!(
        continuation["messages"]
            .to_string()
            .contains("persisted summary evidence"),
        "{continuation}"
    );
    assert!(
        continuation["messages"]
            .to_string()
            .contains("Continue now"),
        "{continuation}"
    );
    host.stop().await;
    provider.stop().await;
}
