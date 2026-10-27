//! Purpose: real isolated Process harness for legacy and typed lifecycle acceptance.
//! Scope: public host methods plus actual SQLite and loopback provider transport.
//! Invariants: all product files live under a temporary directory; no live service or credentials.
//! Contract tests: runtime/tests/legacy_operations.rs, runtime/tests/legacy_model.rs, runtime/tests/legacy_lifecycle.rs

use opencode_runtime::{Process, ProcessOptions};
use serde_json::{Map, Value, json};

#[allow(dead_code)]
pub struct Host {
    pub root: tempfile::TempDir,
    pub directory: std::path::PathBuf,
    pub options: ProcessOptions,
    pub process: Process,
}

impl Host {
    pub async fn start(url: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let directory = base.join("project");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("evidence.txt"),
            "native legacy read evidence\n",
        )
        .unwrap();
        let mut options = ProcessOptions::isolated(&base.join("state"));
        options.version = "legacy-acceptance".into();
        std::fs::create_dir_all(&options.paths.config).unwrap();
        let config = match url {
            Some(url) => {
                json!({"model":"local/local-test","permission":{"*":"allow"},"provider":{"local":{"name":"Loopback","npm":"@ai-sdk/openai-compatible","options":{"baseURL":url,"apiKey":"fixture-key"},"models":{"local-test":{"name":"Local test","tool_call":true,"limit":{"context":32768,"output":1024}}}}}})
            }
            None => json!({"share":"disabled"}),
        };
        std::fs::write(
            options.paths.config.join("opencode.json"),
            config.to_string(),
        )
        .unwrap();
        let process = Process::start(options.clone()).await.unwrap();
        Self {
            root,
            directory,
            options,
            process,
        }
    }
    pub async fn call(
        &self,
        operation: &str,
        id: Option<&str>,
        query: Value,
        payload: Option<Value>,
    ) -> Value {
        self.process
            .legacy_command(
                operation,
                self.directory.clone(),
                id.map(|id| Map::from_iter([("sessionID".into(), json!(id))]))
                    .unwrap_or_default(),
                query.as_object().unwrap().clone(),
                payload,
            )
            .await
            .unwrap()
    }
    pub async fn create(&self, title: &str) -> Value {
        self.call(
            "session.create",
            None,
            json!({}),
            Some(json!({"title":title})),
        )
        .await
    }
    pub async fn messages(&self, id: &str) -> Value {
        self.call("session.messages", Some(id), json!({}), None)
            .await
    }
    pub async fn stop(self) {
        self.process.shutdown().await;
    }
}
