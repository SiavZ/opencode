//! Purpose: production-root acceptance over real SQLite, HTTP APIs and an isolated TCP provider.
//! Scope: integrated model configuration, transport, durable turn completion and interruption.
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- runtime/tests/support/provider.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/repositories/credentials.rs
//! External:
//! |-- credentials::CredentialValue::Key
//! |-- credentials::create
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! |-- tokio::process::Command::new
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: no injected runner, fake tool materializer, user credentials or live model calls;
//! becoming idle is insufficient unless the completed assistant projection is also observed.
//! Contract tests: this file

#[path = "support/provider.rs"]
mod provider;

use std::time::Duration;

use opencode_runtime::{ListenOptions, Listener, Process, ProcessOptions};
use provider::{Provider, Reply};
use serde_json::{Value, json};

struct Host {
    _root: tempfile::TempDir,
    directory: String,
    options: ProcessOptions,
    process: Process,
    listener: Listener,
    client: reqwest::Client,
}

impl Host {
    async fn start(provider: &Provider) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let directory = base.join("project");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("evidence.txt"),
            "real native read evidence\n",
        )
        .unwrap();
        let options = ProcessOptions::isolated(&base.join("opencode"));
        std::fs::create_dir_all(&options.paths.config).unwrap();
        std::fs::write(options.paths.config.join("opencode.json"), json!({
            "$schema": "https://opencode.ai/config.json",
            "model": "local/local-test",
            "providers": {"local": {
                "name": "Controlled loopback",
                "api": {"type":"aisdk", "package":"@ai-sdk/openai-compatible", "url":provider.url},
                "request": {"body":{"apiKey":"local-fixture-key"}},
                "models": {"local-test": {
                    "name":"Local test",
                    "capabilities":{"tools":true,"input":["text"],"output":["text"]},
                    "limit":{"context":32768,"output":1024}
                }}
            }}
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
            _root: root,
            directory: directory.display().to_string(),
            options,
            process,
            listener,
            client: reqwest::Client::new(),
        }
    }

    async fn create(&self, model: &str) -> String {
        let response = self.client.post(format!("{}/api/session", self.listener.url()))
            .json(&json!({"location":{"directory":self.directory},"model":{"providerID":"local","id":model}}))
            .send().await.unwrap();
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
            .json(&json!({"prompt":{"text":"Read evidence.txt and respond"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap()["data"]["sessionID"],
            session
        );
    }

    async fn context(&self, session: &str) -> Value {
        let response = self
            .client
            .get(format!(
                "{}/api/session/{session}/context",
                self.listener.url()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["data"].clone()
    }

    async fn completed(&self, session: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let context = self.context(session).await;
                if context.as_array().unwrap().iter().any(|message| {
                    message["type"] == "assistant" && message["time"]["completed"].is_number()
                }) {
                    let active = self
                        .client
                        .get(format!("{}/api/session/active", self.listener.url()))
                        .send()
                        .await
                        .unwrap()
                        .json::<Value>()
                        .await
                        .unwrap();
                    if active["data"].get(session).is_none() {
                        return context;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("production root must persist a terminal assistant, not merely return idle")
    }

    async fn stop(self) {
        self.listener.stop(true).await;
        self.process.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_production_root_completes_one_provider_turn_and_preserves_it_after_restart() {
    let mut provider = Provider::start(vec![Reply::Text("real composed response")]).await;
    let host = Host::start(&provider).await;
    {
        use opencode_storage_api::{DatabaseExt, TxOptions};
        use opencode_storage_sqlite::repositories::credentials;
        host.process
            .database
            .transaction(TxOptions::default(), |tx| {
                credentials::create(
                    tx,
                    "local",
                    &credentials::CredentialValue::Key {
                        key: "stored-fixture-key".into(),
                        metadata: None,
                    },
                    None,
                )
                .map(|_| ())
            })
            .await
            .unwrap();
    }
    for arguments in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let status = tokio::process::Command::new("git")
            .args(arguments)
            .current_dir(&host.directory)
            .env(
                "GIT_CONFIG_GLOBAL",
                host.options.paths.home.join("empty-gitconfig"),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }
    let models = host
        .process
        .available_models(std::path::PathBuf::from(&host.directory))
        .await
        .unwrap();
    assert!(models.iter().any(|model| model.provider_id.as_str() == "local" && model.id.as_str() == "local-test"));
    let session = host.create("local-test").await;
    host.prompt(&session).await;
    let context = host.completed(&session).await;
    let assistant = context
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["finish"], "stop");
    assert!(
        assistant["snapshot"].is_object(),
        "Git locations must use the real native snapshot service: {assistant}"
    );
    assert!(
        assistant["content"]
            .to_string()
            .contains("real composed response")
    );
    assert_eq!(
        assistant["model"],
        json!({"id":"local-test","providerID":"local","variant":"default"})
    );
    let (headers, request) = tokio::time::timeout(Duration::from_secs(2), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        headers.starts_with("POST /v1/chat/completions "),
        "{headers}"
    );
    assert!(
        headers
            .to_lowercase()
            .contains("authorization: bearer stored-fixture-key")
    );
    assert!(headers.to_lowercase().contains("x-opencode-session-id:"));
    assert_eq!(request["model"], "local-test");
    assert!(
        request.get("apiKey").is_none(),
        "credential leaked into provider body"
    );
    assert!(
        request["messages"].to_string().contains(&host.directory),
        "real built-in environment must reach provider"
    );
    assert!(
        request["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "real tools must be composed"
    );
    let options = host.options.clone();
    host.listener.stop(true).await;
    host.process.shutdown().await;
    let restarted = Process::start(options).await.unwrap();
    let id = opencode_session_api::SessionID::new(session).unwrap();
    let persisted =
        opencode_session_api::operations::SessionService::context(restarted.session.as_ref(), &id)
            .await
            .unwrap();
    assert_eq!(serde_json::to_value(persisted).unwrap(), context);
    restarted.shutdown().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_interrupt_settles_an_active_stream_before_returning_idle() {
    let mut provider = Provider::start(vec![Reply::Hold]).await;
    let host = Host::start(&provider).await;
    let session = host.create("local-test").await;
    let mut events = host
        .client
        .get(format!("{}/api/event", host.listener.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), 200);
    host.prompt(&session).await;
    tokio::time::timeout(Duration::from_secs(10), provider.requests.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut observed = String::new();
        while let Some(chunk) = events.chunk().await.unwrap() {
            observed.push_str(&String::from_utf8_lossy(&chunk));
            if observed.contains("session.next.text.delta") && observed.contains("partial") {
                return;
            }
        }
        panic!("live event stream ended before the provider text delta");
    })
    .await
    .expect("ephemeral text must be observable through the public live SSE stream");
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        host.client
            .post(format!(
                "{}/api/session/{session}/interrupt",
                host.listener.url()
            ))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 204);
    let context = host.completed(&session).await;
    let assistant = context
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["finish"], "error");
    assert!(
        assistant["content"].to_string().contains("partial"),
        "interrupt must flush accumulated ephemeral text into durable content: {assistant}"
    );
    assert!(
        assistant["error"]
            .to_string()
            .to_lowercase()
            .contains("interrupt"),
        "{assistant}"
    );
    drop(events);
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_provider_rejection_is_a_durable_error_not_a_false_success() {
    let mut provider = Provider::start(vec![Reply::Failure]).await;
    let host = Host::start(&provider).await;
    let session = host.create("local-test").await;
    host.prompt(&session).await;
    let context = host.completed(&session).await;
    let assistant = context
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["finish"], "error");
    assert!(
        assistant["error"]
            .to_string()
            .contains("controlled rejection"),
        "{assistant}"
    );
    assert!(provider.requests.recv().await.is_some());
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_tools_execute_native_read_and_continue_with_its_actual_output() {
    let mut provider = Provider::start(vec![
        Reply::Tool {
            name: "read",
            arguments: json!({"path":"evidence.txt"}),
        },
        Reply::Text("read completed"),
    ])
    .await;
    let host = Host::start(&provider).await;
    let session = host.create("local-test").await;
    host.prompt(&session).await;
    let context = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let context = host.context(&session).await;
            if context.to_string().contains("read completed") {
                return context;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tool call must settle and continue through the real registry");
    assert!(
        context.to_string().contains("real native read evidence"),
        "{context}"
    );
    let _ = provider.requests.recv().await.unwrap();
    let (_, continuation) = provider.requests.recv().await.unwrap();
    assert!(
        continuation["messages"]
            .to_string()
            .contains("real native read evidence")
    );
    host.completed(&session).await;
    host.stop().await;
    provider.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_native_write_enforces_the_location_permission_boundary() {
    for allowed in [true, false] {
        let outside = tempfile::tempdir().unwrap();
        let path = if allowed {
            "created.txt".to_owned()
        } else {
            outside.path().join("outside.txt").display().to_string()
        };
        let provider = Provider::start(vec![
            Reply::Tool {
                name: "write",
                arguments: json!({"path":path,"content":"production write evidence\n"}),
            },
            Reply::Text("write attempt settled"),
        ])
        .await;
        let host = Host::start(&provider).await;
        let session = host.create("local-test").await;
        host.prompt(&session).await;
        if !allowed {
            let request = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let pending = host
                        .process
                        .pending_permissions(host.directory.clone().into())
                        .await
                        .unwrap();
                    if let Some(request) = pending
                        .into_iter()
                        .find(|request| request.session_id == session)
                    {
                        break request;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            host.process
                .reply_permission(
                    host.directory.clone().into(),
                    opencode_permissions::ReplyInput {
                        request_id: request.id,
                        reply: opencode_permissions::Reply::Reject,
                        message: None,
                    },
                )
                .await
                .unwrap();
        }
        let context = host.completed(&session).await;
        let target = std::path::Path::new(&host.directory).join(path);
        if allowed {
            assert_eq!(
                std::fs::read_to_string(target).unwrap(),
                "production write evidence\n"
            );
            assert!(
                context.to_string().contains("Created file successfully"),
                "{context}"
            );
        } else {
            assert!(
                !target.exists(),
                "outside-location tool request wrote a file"
            );
            assert!(context.to_string().contains("Unable to write"), "{context}");
        }
        assert!(
            context.to_string().contains("write attempt settled"),
            "{context}"
        );
        host.stop().await;
        provider.stop().await;
    }
}
