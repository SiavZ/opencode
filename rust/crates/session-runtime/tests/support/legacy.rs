//! Purpose: shared fixtures for legacy Session tests: a migrated SQLite harness with legacy
//! projectors, a seeded Session, fixture agent/provider/command registries, a scripted
//! `LegacyLoop`, a recording `PromptTools`, a tempdir-backed `FileSystem` and the real native process
//! runner.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/error.rs
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/process/mod.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/tests/support/sql.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- sql::Harness
//! |-- sql::open
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::sync::Notify
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/compaction_legacy.rs
//! |-- session-runtime/tests/legacy_command.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! |-- session-runtime/tests/legacy_shell.rs
//! |-- session-runtime/tests/summary.rs
//! `-- session-runtime/tests/title.rs
//! Invariants: every double is labeled fixture; the database, projectors, run state, store and
//! process runner are the real implementations.
//! Contract tests: session-runtime/tests/legacy_*.rs
#![allow(dead_code)]

use super::sql;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_platform_api::error::{PlatformError, PlatformErrorKind};
use opencode_platform_api::fs::{FileSystem, FileType, Metadata};
use opencode_platform_native::process::NativeProcessRunner;
use opencode_session_runtime::legacy::ports::{
    CommandInfo, LegacyAgents, LegacyCommands, LegacyLoop, LegacyProvider, PortError, PromptTools,
    ReadContext, ReadOutput, ShareRemote,
};
use opencode_session_runtime::legacy::projectors::register_projectors;
use opencode_session_runtime::legacy::run_state::{RunState, SessionStatus};
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::legacy::types::{
    AgentInfo, LegacyError, ModelRef, Object, WithParts,
};
use opencode_session_runtime::legacy::{Deps, Instance, LegacyConfig, LegacySession};
use opencode_storage_api::{DatabaseExt, Sql, TxOptions};
use serde_json::{Value, json};
use tokio::sync::Notify;

pub const SESSION: &str = "ses_legacy";

/// Fixture agents: `build` (primary, default), `plan`, `explore` (subagent), `hidden` (hidden).
pub struct FixtureAgents;

pub fn agent(name: &str, mode: &str) -> AgentInfo {
    AgentInfo {
        name: name.into(),
        mode: mode.into(),
        ..AgentInfo::default()
    }
}

#[async_trait]
impl LegacyAgents for FixtureAgents {
    async fn get(&self, name: &str) -> Result<Option<AgentInfo>, LegacyError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|agent| agent.name == name))
    }
    async fn list(&self) -> Result<Vec<AgentInfo>, LegacyError> {
        let mut hidden = agent("hidden", "primary");
        hidden.hidden = Some(true);
        let mut denied = agent("denied", "primary");
        denied.permission = vec![
            serde_json::from_value(
                json!({ "permission": "task", "pattern": "*", "action": "deny" }),
            )
            .unwrap(),
        ];
        Ok(vec![
            agent("build", "primary"),
            agent("plan", "primary"),
            agent("explore", "subagent"),
            hidden,
            denied,
        ])
    }
    async fn default_info(&self) -> Result<AgentInfo, LegacyError> {
        Ok(agent("build", "primary"))
    }
}

/// Fixture provider: `test/model` exists, everything else is not found.
pub struct FixtureProvider;

#[async_trait]
impl LegacyProvider for FixtureProvider {
    async fn get_model(&self, provider_id: &str, model_id: &str) -> Result<Object, LegacyError> {
        if provider_id == "test" && model_id == "model" {
            return Ok(
                serde_json::from_value(json!({ "id": "model", "providerID": "test" })).unwrap(),
            );
        }
        Err(LegacyError::ModelNotFound {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            suggestions: vec!["test/model".into()],
        })
    }
    async fn default_model(&self) -> Result<ModelRef, LegacyError> {
        Ok(ModelRef::new("test", "model"))
    }
}

/// Fixture loop: records runs; blocks until `release` when `gate` is set.
#[derive(Default)]
pub struct ScriptedLoop {
    pub runs: Mutex<Vec<String>>,
    pub gate: Mutex<Option<Arc<Notify>>>,
    pub started: Notify,
}

#[async_trait]
impl LegacyLoop for ScriptedLoop {
    async fn run(&self, session_id: &str) -> Result<WithParts, LegacyError> {
        self.runs.lock().unwrap().push(session_id.into());
        self.started.notify_waiters();
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Ok(WithParts {
            info: serde_json::from_value(json!({ "id": "msg_reply", "role": "assistant" }))
                .unwrap(),
            parts: vec![],
        })
    }
}

/// Fixture tools: Read returns a canned output, plugin hooks record their names.
#[derive(Default)]
pub struct FixtureTools {
    pub reads: Mutex<Vec<(Value, ReadContext)>>,
    pub hooks: Mutex<Vec<String>>,
    pub fail_read: Mutex<Option<String>>,
}

#[async_trait]
impl PromptTools for FixtureTools {
    async fn read(&self, args: Value, ctx: ReadContext) -> Result<ReadOutput, PortError> {
        self.reads.lock().unwrap().push((args, ctx));
        if let Some(message) = self.fail_read.lock().unwrap().clone() {
            return Err(PortError::Failed(message));
        }
        Ok(ReadOutput {
            output: "<file>contents</file>".into(),
            attachments: vec![],
        })
    }
    async fn read_resource(&self, _: &str, uri: &str) -> Result<Option<Value>, PortError> {
        Ok(Some(json!({ "contents": [
            { "text": format!("text of {uri}") },
            { "blob": "aGVsbG8=", "mimeType": "image/png", "uri": "res.png" },
            { "blob": "aGVsbG8=", "mimeType": "application/zip" },
        ] })))
    }
    async fn document_symbols(&self, _: &str) -> Result<Vec<Value>, PortError> {
        Ok(vec![])
    }
    async fn normalize_image(&self, _: &Object) -> Result<Option<Object>, PortError> {
        Ok(None)
    }
    async fn trigger(&self, name: &str, _: Value, output: Value) -> Result<Value, PortError> {
        self.hooks.lock().unwrap().push(name.into());
        Ok(output)
    }
}

/// Fixture commands keyed by name.
#[derive(Default)]
pub struct FixtureCommands(pub Vec<CommandInfo>);

#[async_trait]
impl LegacyCommands for FixtureCommands {
    async fn get(&self, name: &str) -> Result<Option<CommandInfo>, LegacyError> {
        Ok(self.0.iter().find(|command| command.name == name).cloned())
    }
    async fn list(&self) -> Result<Vec<CommandInfo>, LegacyError> {
        Ok(self.0.clone())
    }
}

/// Fixture share remote returning a deterministic URL.
#[derive(Default)]
pub struct FixtureShare {
    pub removed: Mutex<Vec<String>>,
}

#[async_trait]
impl ShareRemote for FixtureShare {
    async fn create(&self, session_id: &str) -> Result<String, PortError> {
        Ok(format!("https://share.test/{session_id}"))
    }
    async fn remove(&self, session_id: &str) -> Result<(), PortError> {
        self.removed.lock().unwrap().push(session_id.into());
        Ok(())
    }
}

/// Fixture filesystem over a real temp directory (std I/O, absolute paths).
#[derive(Debug)]
pub struct TempFs;

fn io(path: &Path, error: std::io::Error) -> PlatformError {
    let kind = if error.kind() == std::io::ErrorKind::NotFound {
        PlatformErrorKind::NotFound
    } else {
        PlatformErrorKind::Io
    };
    PlatformError::new(kind, "fs", path.display().to_string(), error.to_string())
}

#[async_trait]
impl FileSystem for TempFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, PlatformError> {
        std::fs::read(path).map_err(|e| io(path, e))
    }
    async fn read_to_string(&self, path: &Path) -> Result<String, PlatformError> {
        std::fs::read_to_string(path).map_err(|e| io(path, e))
    }
    async fn metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        let meta = std::fs::metadata(path).map_err(|e| io(path, e))?;
        Ok(Metadata {
            file_type: if meta.is_dir() {
                FileType::Directory
            } else {
                FileType::File
            },
            size: meta.len(),
            modified: None,
            identity: None,
        })
    }
    async fn symlink_metadata(&self, path: &Path) -> Result<Metadata, PlatformError> {
        self.metadata(path).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, PlatformError> {
        Ok(path.exists())
    }
    async fn real_path(&self, path: &Path) -> Result<PathBuf, PlatformError> {
        std::fs::canonicalize(path).map_err(|e| io(path, e))
    }
    async fn read_dir(&self, _: &Path) -> Result<Vec<(String, FileType)>, PlatformError> {
        Ok(vec![])
    }
    async fn create_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::create_dir_all(path).map_err(|e| io(path, e))
    }
    async fn write_with_dirs(&self, path: &Path, content: &[u8]) -> Result<(), PlatformError> {
        std::fs::write(path, content).map_err(|e| io(path, e))
    }
    async fn remove_file(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::remove_file(path).map_err(|e| io(path, e))
    }
    async fn remove_dir_all(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::remove_dir_all(path).map_err(|e| io(path, e))
    }
    async fn rename(&self, from: &Path, to: &Path) -> Result<(), PlatformError> {
        std::fs::rename(from, to).map_err(|e| io(from, e))
    }
}

pub struct Ctx {
    pub h: sql::Harness,
    pub worktree: tempfile::TempDir,
    pub legacy: LegacySession,
    pub run_loop: Arc<ScriptedLoop>,
    pub tools: Arc<FixtureTools>,
    pub share: Arc<FixtureShare>,
}

pub struct Options {
    pub commands: Vec<CommandInfo>,
    pub config: LegacyConfig,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            commands: vec![],
            config: LegacyConfig {
                shell: "/bin/sh".into(),
                ..LegacyConfig::default()
            },
        }
    }
}

pub async fn setup() -> Ctx {
    setup_with(Options::default()).await
}

pub async fn setup_with(options: Options) -> Ctx {
    let h = sql::open().await;
    register_projectors(h.store.as_ref());
    h.seed_session(SESSION).await;
    let worktree = tempfile::tempdir().unwrap();
    let store = LegacyStore::new(h.events.clone());
    let run_state = RunState::new(SessionStatus::new(store.clone()), None);
    let run_loop = Arc::new(ScriptedLoop::default());
    let tools = Arc::new(FixtureTools::default());
    let share = Arc::new(FixtureShare::default());
    let legacy = LegacySession::new(Deps {
        store,
        run_state,
        agents: Arc::new(FixtureAgents),
        provider: Arc::new(FixtureProvider),
        run_loop: run_loop.clone(),
        tools: tools.clone(),
        commands: Arc::new(FixtureCommands(options.commands)),
        share: share.clone(),
        fs: Arc::new(TempFs),
        process: Arc::new(NativeProcessRunner::new()),
        instance: Instance {
            directory: worktree.path().to_path_buf(),
            worktree: worktree.path().to_path_buf(),
            home: worktree.path().join("home"),
        },
        config: options.config,
    });
    Ctx {
        h,
        worktree,
        legacy,
        run_loop,
        tools,
        share,
    }
}

impl Ctx {
    pub async fn exec(&self, sql: &'static str, params: Vec<opencode_storage_api::Value>) {
        self.h
            .db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(sql, &params)
            })
            .await
            .unwrap();
    }

    pub async fn messages(&self) -> Vec<WithParts> {
        self.legacy.store().messages(SESSION).await.unwrap()
    }
}
