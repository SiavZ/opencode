//! Purpose: production compaction acceptance through real Process HTTP, SQLite and loopback LLM.
//! Scope: configured automatic summary turns, cancellation, projection and continuation.
//! Uses:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/tests/support/provider.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- reqwest::Client
//! |-- serde_json::{Value, json}
//! `-- tokio::time
//! Used by:
//! `-- none, entrypoint
//! Invariants: real configuration selects the actual runner and provider transport. No injected
//! runner, event source or model client. All HTTP traffic remains on loopback with isolated paths.
//! Contract tests: this file

#[allow(dead_code)]
#[path = "support/provider.rs"]
mod provider;

use std::time::Duration;

use opencode_runtime::{ListenOptions, Listener, Process, ProcessOptions};
use opencode_storage_api::DatabaseExt;
use provider::{Provider, Reply};
use serde_json::{Value, json};

struct Host {
    _root: tempfile::TempDir,
    process: Process,
    listener: Listener,
    client: reqwest::Client,
    session: String,
}

impl Host {
    async fn start(provider: &Provider, auto: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let project = directory.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let options = ProcessOptions::isolated(&directory.join("opencode"));
        std::fs::create_dir_all(&options.paths.config).unwrap();
        std::fs::write(
            options.paths.config.join("opencode.json"),
            json!({
                "$schema":"https://opencode.ai/config.json",
                "model":"local/local-test",
                "compaction":{"auto":auto,"buffer":90000,"keep":{"tokens":0}},
                "providers":{"local":{
                    "name":"Controlled loopback",
                    "api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","url":provider.url},
                    "request":{"body":{"apiKey":"local-fixture-key"}},
                    "models":{"local-test":{
                        "name":"Local test",
                        "capabilities":{"tools":true,"input":["text"],"output":["text"]},
                        "limit":{"context":100000,"output":1024}
                    }}
                }}
            })
            .to_string(),
        )
        .unwrap();
        let process = Process::start(options).await.unwrap();
        let listener = process
            .serve(
                ListenOptions {
                    hostname: "127.0.0.1".into(),
                    port: 0,
                },
                Vec::new(),
            )
            .await
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client.post(format!("{}/api/session", listener.url()))
            .json(&json!({"location":{"directory":project.display().to_string()},"model":{"providerID":"local","id":"local-test"}}))
            .send().await.unwrap();
        assert_eq!(response.status(), 200);
        let session = response.json::<Value>().await.unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            _root: root,
            process,
            listener,
            client,
            session,
        }
    }

    async fn prompt(&self) {
        let response = self
            .client
            .post(format!(
                "{}/api/session/{}/prompt",
                self.listener.url(),
                self.session
            ))
            .json(&json!({"prompt":{"text":"long-request ".repeat(4000)}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    async fn context(&self) -> Value {
        let response = self
            .client
            .get(format!(
                "{}/api/session/{}/context",
                self.listener.url(),
                self.session
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["data"].clone()
    }

    async fn idle(&self) -> bool {
        let response = self
            .client
            .get(format!("{}/api/session/active", self.listener.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["data"]
            .get(&self.session)
            .is_none()
    }

    async fn compaction_events(&self) -> Vec<String> {
        let session = self.session.clone();
        self.process.database.read(move |tx| {
            tx.query("SELECT type FROM event WHERE aggregate_id = ? AND type LIKE 'session.next.compaction.%' ORDER BY seq", &[session.into()])
        }).await.unwrap().iter().map(|row| row.get::<String>("type").unwrap()).collect()
    }

    async fn stop(self) {
        self.listener.stop(true).await;
        self.process.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_interrupt_cancels_held_summary_without_publishing_ended() {
    let mut provider = Provider::start(vec![Reply::Hold]).await;
    let host = Host::start(&provider, true).await;
    host.prompt().await;
    let (headers, summary) =
        tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
            .await
            .unwrap()
            .unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions "));
    assert!(
        summary["messages"]
            .to_string()
            .contains("Create a new anchored summary"),
        "configured compaction must execute before a normal provider turn"
    );
    assert_eq!(
        host.compaction_events().await,
        ["session.next.compaction.started.1"]
    );
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        host.client
            .post(format!(
                "{}/api/session/{}/interrupt",
                host.listener.url(),
                host.session
            ))
            .send(),
    )
    .await
    .expect("held compaction must be interruptible")
    .unwrap();
    assert_eq!(response.status(), 204);
    assert!(
        host.idle().await,
        "interrupt must await the complete active ownership chain"
    );
    assert_eq!(
        host.compaction_events().await,
        ["session.next.compaction.started.1"]
    );
    assert!(
        !host
            .context()
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "compaction")
    );
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_interrupt_cancels_overflow_summary_with_automatic_compaction_disabled() {
    let mut provider = Provider::start(vec![Reply::Overflow, Reply::Hold]).await;
    let host = Host::start(&provider, false).await;
    host.prompt().await;
    let (_, first) = tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !first["messages"]
            .to_string()
            .contains("Create a new anchored summary")
    );
    let (_, summary) = tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        summary["messages"]
            .to_string()
            .contains("Create a new anchored summary"),
        "real context overflow must enter recovery summary even when auto is disabled"
    );
    assert_eq!(
        host.compaction_events().await,
        ["session.next.compaction.started.1"]
    );
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        host.client
            .post(format!(
                "{}/api/session/{}/interrupt",
                host.listener.url(),
                host.session
            ))
            .send(),
    )
    .await
    .expect("overflow summary must be interruptible")
    .unwrap();
    assert_eq!(response.status(), 204);
    assert!(host.idle().await);
    assert_eq!(
        host.compaction_events().await,
        ["session.next.compaction.started.1"]
    );
    assert!(
        !host
            .context()
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "compaction")
    );
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_summary_projects_and_continues_with_reloaded_history() {
    let mut provider = Provider::start(vec![
        Reply::Text("## Objective\n- actual composed summary"),
        Reply::Text("continued after actual summary"),
    ])
    .await;
    let host = Host::start(&provider, true).await;
    host.prompt().await;
    let (_, summary) = tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        summary["messages"]
            .to_string()
            .contains("Create a new anchored summary")
    );
    let (_, continuation) = tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        continuation["messages"]
            .to_string()
            .contains("actual composed summary"),
        "continuation must reload the projected summary"
    );
    let context = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let context = host.context().await;
            if context.as_array().unwrap().iter().any(|message| {
                message["type"] == "assistant" && message["time"]["completed"].is_number()
            }) && host.idle().await
            {
                break context;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        host.compaction_events().await,
        [
            "session.next.compaction.started.1",
            "session.next.compaction.ended.1"
        ]
    );
    assert!(
        context
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "compaction"
                && message["summary"] == "## Objective\n- actual composed summary")
    );
    let assistant = context
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["finish"], "stop");
    assert!(
        assistant["content"]
            .to_string()
            .contains("continued after actual summary")
    );
    host.stop().await;
    provider.stop().await;
}
