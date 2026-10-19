//! Purpose: invoke process-wide registered tools through the actual embedded HTTP runner.
//! Scope: two isolated Locations, real loopback model transport and durable assistant history.
//! Uses:
//! rust/crates/
//! |-- sdk-embedded/src/lib.rs
//! |-- sdk-embedded/tests/support/embedded_provider.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: no fake runner, materializer, user directory or live model provider.
//! Contract tests: self
#![cfg(feature = "embedded")]

#[path = "support/embedded_provider.rs"]
mod provider;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use opencode_sdk_embedded::{OpenCode, ProcessOptions, RequestDescriptor};
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};

struct Echo(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Echo {
    fn description(&self) -> &str {
        "Return embedded acceptance evidence"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
    }
    async fn execute(
        &self,
        input: Value,
        _context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new(
            json!({}),
            vec![ToolContent::text(format!(
                "embedded-evidence:{}",
                input["text"].as_str().unwrap()
            ))],
        ))
    }
}

fn request(method: &'static str, path: String, body: Option<Value>) -> RequestDescriptor {
    RequestDescriptor {
        method,
        path,
        body,
        query: Vec::new(),
        headers: Vec::new(),
        success_status: 200,
        declared_statuses: &[400, 404],
        empty: false,
    }
}

#[tokio::test]
async fn registered_application_tool_executes_in_two_real_location_runners() {
    let root = tempfile::tempdir().unwrap();
    let provider = provider::Provider::start().await;
    let options = ProcessOptions::isolated(&root.path().join("host"));
    std::fs::create_dir_all(&options.paths.config).unwrap();
    std::fs::write(options.paths.config.join("opencode.json"), json!({
        "model":"local/local-test",
        "providers":{"local":{
            "name":"Controlled loopback",
            "api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","url":provider.url},
            "request":{"body":{"apiKey":"local-fixture-key"}},
            "models":{"local-test":{"name":"Local test","capabilities":{"tools":true,"input":["text"],"output":["text"]},"limit":{"context":32768,"output":1024}}}
        }}
    }).to_string()).unwrap();
    let host = OpenCode::create_with_options(options).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    for name in ["one", "two"] {
        let directory = root.path().join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let created = host.client.request(request("POST", "/api/session".into(), Some(json!({"location":{"directory":directory},"model":{"providerID":"local","id":"local-test"}})))).await.unwrap();
        let id = created["data"]["id"].as_str().unwrap();
        if name == "one" {
            host.tools
                .register(vec![(
                    "embedded_echo".into(),
                    Arc::new(Echo(calls.clone())),
                )])
                .unwrap();
        }
        host.client
            .request(request(
                "POST",
                format!("/api/session/{id}/prompt"),
                Some(json!({"prompt":{"text":"Use embedded_echo then finish"}})),
            ))
            .await
            .unwrap();
        let context = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let context = host
                    .client
                    .request(request("GET", format!("/api/session/{id}/context"), None))
                    .await
                    .unwrap();
                let active = host
                    .client
                    .request(request("GET", "/api/session/active".into(), None))
                    .await
                    .unwrap();
                if active["data"].get(id).is_none()
                    && context["data"].as_array().unwrap().iter().any(|message| {
                        message["type"] == "assistant" && message["time"]["completed"].is_number()
                    })
                {
                    break context;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real embedded runner must complete durably");
        assert!(
            context
                .to_string()
                .contains("embedded-evidence:native-call"),
            "{context}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let requests = provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    for index in [0, 2] {
        assert!(
            requests[index]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "embedded_echo")
        );
        assert!(
            requests[index + 1]["messages"]
                .to_string()
                .contains("embedded-evidence:native-call")
        );
    }
    host.shutdown().await;
    provider.stop().await;
}
