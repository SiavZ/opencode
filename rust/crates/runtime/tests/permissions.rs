//! Purpose: prove real permission pending/reply resumes the native tool/provider and persists grants across restart.
//! Scope: isolated Process, SQLite, HTTP and controlled loopback provider, no injected authorization fixtures.
//! Uses:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! |-- runtime/src/lib.rs
//! `-- runtime/tests/support/provider.rs
//! External:
//! |-- futures::StreamExt
//! |-- reqwest::Client
//! |-- serde_json::json
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: no file content reaches a provider before approval; cancellation removes pending; saved grants cannot override denies.
//! Contract tests: this file

#[path = "permissions/path_security.rs"]
mod path_security;
#[path = "support/provider.rs"]
mod provider;
#[path = "permissions/provider_failure.rs"]
mod provider_failure;
#[path = "permissions/questions.rs"]
mod questions;

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use opencode_event_api::{Definition, EventStore};
use opencode_permissions::{AssertInput, PermissionError, Reply, ReplyInput, SavedGrants};
use opencode_runtime::{ListenOptions, Listener, Process, ProcessOptions};
use opencode_schema::{location::Ref, values::AbsolutePath};
use provider::Provider;
use serde_json::{Value, json};

struct Host {
    root: tempfile::TempDir,
    directory: PathBuf,
    options: ProcessOptions,
    process: Process,
    listener: Listener,
    client: reqwest::Client,
}

impl Host {
    async fn start(provider: &Provider) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap().join("project");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("evidence.txt"), "native approved evidence\n").unwrap();
        std::fs::write(directory.join("secret.txt"), "denied secret content\n").unwrap();
        let options = ProcessOptions::isolated(&root.path().join("host"));
        std::fs::create_dir_all(&options.paths.config).unwrap();
        std::fs::write(options.paths.config.join("opencode.json"), json!({
            "model":"local/local-test",
            "permissions":[{"action":"read","resource":"*","effect":"ask"},{"action":"read","resource":"secret.txt","effect":"deny"}],
            "providers":{"local":{"name":"Local fixture","api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","url":provider.url},"request":{"body":{"apiKey":"local-only"}},"models":{"local-test":{"name":"Local test","capabilities":{"tools":true,"input":["text"],"output":["text"]},"limit":{"context":32768,"output":1024}}}}}
        }).to_string()).unwrap();
        let process = Process::start(options.clone()).await.unwrap();
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
        Self {
            root,
            directory,
            options,
            process,
            listener,
            client: reqwest::Client::new(),
        }
    }

    async fn create(&self) -> String {
        let response = self.client.post(format!("{}/api/session", self.listener.url())).json(&json!({"location":{"directory":self.directory},"model":{"providerID":"local","id":"local-test"}})).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn prompt(&self, session: &str) {
        let response = self
            .client
            .post(format!(
                "{}/api/session/{session}/prompt",
                self.listener.url()
            ))
            .json(&json!({"prompt":{"text":"Read evidence"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    async fn pending(&self) -> opencode_permissions::PermissionRequest {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let pending = self
                    .process
                    .pending_permissions(self.directory.clone())
                    .await
                    .unwrap();
                if let Some(request) = pending.into_iter().next() {
                    return request;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runner must expose a real pending permission")
    }

    async fn context(&self, session: &str) -> Value {
        self.client
            .get(format!(
                "{}/api/session/{session}/context",
                self.listener.url()
            ))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["data"]
            .clone()
    }

    async fn completed(&self, session: &str, needle: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let context = self.context(session).await;
                if context.to_string().contains(needle) {
                    return context;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("provider continuation must be projected")
    }

    async fn stop(self) {
        self.listener.stop(true).await;
        self.process.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_http_reply_continues_real_native_tool_and_provider() {
    let mut provider = Provider::start(vec![
        provider::Reply::Tool {
            name: "read",
            arguments: json!({"path":"evidence.txt"}),
        },
        provider::Reply::Text("approved continuation"),
    ])
    .await;
    let host = Host::start(&provider).await;
    let mut asked = host
        .process
        .events
        .subscribe(&Definition::ephemeral("permission.v2.asked"));
    let mut replied = host
        .process
        .events
        .subscribe(&Definition::ephemeral("permission.v2.replied"));
    let session = host.create().await;
    host.prompt(&session).await;
    let pending = host.pending().await;
    assert_eq!(pending.session_id, session);
    assert_eq!(pending.action, "read");
    assert_eq!(pending.resources, ["evidence.txt"]);
    assert!(pending.source.is_some());
    assert_eq!(asked.next().await.unwrap().unwrap().data["id"], pending.id);
    provider.requests.recv().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), provider.requests.recv())
            .await
            .is_err(),
        "provider continued before permission was granted"
    );
    assert!(
        !host
            .context(&session)
            .await
            .to_string()
            .contains("native approved evidence")
    );
    let listed = host
        .client
        .get(format!(
            "{}/api/session/{session}/permission",
            host.listener.url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    assert_eq!(
        listed.json::<Value>().await.unwrap()["data"][0]["id"],
        pending.id
    );
    let response = host
        .client
        .post(format!(
            "{}/api/session/{session}/permission/{}/reply",
            host.listener.url(),
            pending.id
        ))
        .json(&json!({"reply":"once"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(
        replied.next().await.unwrap().unwrap().data["requestID"],
        pending.id
    );
    let context = host.completed(&session, "approved continuation").await;
    assert!(context.to_string().contains("native approved evidence"));
    let (_, continuation) = provider.requests.recv().await.unwrap();
    assert!(
        continuation["messages"]
            .to_string()
            .contains("native approved evidence")
    );
    assert!(
        host.process
            .pending_permissions(host.directory.clone())
            .await
            .unwrap()
            .is_empty()
    );
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_always_survives_restart_but_never_overrides_configured_deny() {
    let provider = Provider::start(vec![]).await;
    let host = Host::start(&provider).await;
    let session = host.create().await;
    let location = Ref {
        directory: AbsolutePath::new(host.directory.display().to_string()),
        workspace_id: None,
    };
    let lease = host.process.locations.get(&location).await.unwrap();
    let service = lease.permissions.service.clone();
    let mut input = AssertInput::new(&session, "read", vec!["evidence.txt".into()]);
    input.save = Some(vec!["*".into()]);
    let waiting = tokio::spawn({
        let service = service.clone();
        let input = input.clone();
        async move { service.assert(input).await }
    });
    let pending = host.pending().await;
    host.process
        .reply_permission(
            host.directory.clone(),
            ReplyInput {
                request_id: pending.id,
                reply: Reply::Always,
                message: None,
            },
        )
        .await
        .unwrap();
    waiting.await.unwrap().unwrap();
    let saved = lease
        .permissions
        .saved
        .list(Some(lease.info.project.id.as_str()))
        .await
        .unwrap();
    assert_eq!(saved.len(), 1);
    let saved_id = saved[0].id.clone();
    assert!(matches!(
        service
            .assert(AssertInput::new(
                &session,
                "read",
                vec!["secret.txt".into()]
            ))
            .await,
        Err(PermissionError::Blocked { .. })
    ));
    drop(lease);
    drop(service);
    host.listener.stop(true).await;
    host.process.shutdown().await;
    let restarted = Process::start(host.options.clone()).await.unwrap();
    let lease = restarted.locations.get(&location).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        lease.permissions.service.assert(input.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        lease.permissions.saved.list(None).await.unwrap()[0].id,
        saved_id
    );
    assert!(matches!(
        lease
            .permissions
            .service
            .assert(AssertInput::new(
                &session,
                "read",
                vec!["secret.txt".into()]
            ))
            .await,
        Err(PermissionError::Blocked { .. })
    ));
    let other = host.root.path().join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    // Project identity for non-git directories is shared by the baseline. Explicitly assert the SQL project filter instead.
    assert!(
        SavedGrants::list(lease.permissions.saved.as_ref(), "prj_unrelated")
            .await
            .unwrap()
            .is_empty()
    );
    lease.permissions.saved.remove(&saved_id).await.unwrap();
    assert!(lease.permissions.saved.list(None).await.unwrap().is_empty());
    let pending = lease.permissions.service.ask(input).await.unwrap();
    assert_eq!(pending.effect, opencode_permissions::Effect::Ask);
    drop(lease);
    restarted.shutdown().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_native_tool_never_reads_content_and_interrupt_cleans_pending() {
    for reject in [true, false] {
        let provider = Provider::start(vec![
            provider::Reply::Tool {
                name: "read",
                arguments: json!({"path":"evidence.txt"}),
            },
            provider::Reply::Text("rejected continuation"),
        ])
        .await;
        let host = Host::start(&provider).await;
        let session = host.create().await;
        host.prompt(&session).await;
        let pending = host.pending().await;
        if reject {
            host.process
                .reply_permission(
                    host.directory.clone(),
                    ReplyInput {
                        request_id: pending.id,
                        reply: Reply::Reject,
                        message: Some("do not read it".into()),
                    },
                )
                .await
                .unwrap();
            let context = host.completed(&session, "rejected continuation").await;
            // core/tool/read.ts deliberately hides permission details behind this exact error.
            assert!(
                context.to_string().contains("Unable to read evidence.txt"),
                "{context}"
            );
            assert!(!context.to_string().contains("native approved evidence"));
        } else {
            let response = host
                .client
                .post(format!(
                    "{}/api/session/{session}/interrupt",
                    host.listener.url()
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 204);
        }
        assert!(
            host.process
                .pending_permissions(host.directory.clone())
                .await
                .unwrap()
                .is_empty()
        );
        host.stop().await;
        provider.stop().await;
    }
}
