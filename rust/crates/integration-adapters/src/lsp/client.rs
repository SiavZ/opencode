//! Purpose: one running LSP client: server-request handlers, initialize handshake with baseline
//! capabilities, document sync (didOpen / didChange full or incremental / didSave / didClose with
//! versioning) and joined shutdown. Ports packages/opencode/src/lsp/client.ts:123-268,543-648.
//! Scope: one (server, root) process. Diagnostic pull/wait logic lives in diagnostics.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/jsonrpc.rs
//! |-- integration-adapters/src/lsp/text.rs
//! |-- integration-api/src/language.rs
//! `-- integration-api/src/lib.rs
//! External:
//! |-- broadcast::Sender
//! |-- broadcast::channel
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- tokio::fs::read_to_string
//! |-- tokio::io::AsyncReadExt
//! |-- tokio::io::BufReader
//! |-- tokio::process::Child
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::Mutex::new
//! |-- tokio::sync::broadcast
//! |-- tokio::sync::watch
//! |-- tokio::task::JoinSet
//! |-- tracing::debug
//! |-- watch::Sender
//! `-- watch::channel
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/diagnostics.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/mod.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! `-- integration-adapters/src/lsp/service.rs
//! Invariants: didOpen is version 0 and clears cached diagnostics, every re-open bumps the version
//! by one and sends didChange, incremental sync replaces the whole previous text range in UTF-16
//! units, shutdown kills and waits for the process and joins the reader and stderr tasks.
//! Contract tests: integration-adapters/tests/lsp_client.rs

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opencode_integration_api::IntegrationError;
use opencode_integration_api::language::Diagnostic;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinSet;

use super::jsonrpc::{Connection, Handler};
use super::text::{absolute, end_position, file_uri, language_id, uri_to_path};

const FILE_CHANGE_CREATED: u8 = 1;
const FILE_CHANGE_CHANGED: u8 = 2;
const TEXT_DOCUMENT_SYNC_INCREMENTAL: i64 = 2;

/// A spawned server process plus its `initializationOptions` (server.ts `Handle`).
pub struct Handle {
    pub child: Child,
    pub initialization: Option<Map<String, Value>>,
}

/// Client timing knobs. Defaults are the baseline constants (client.ts:13-18).
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub initialize: Duration,
    pub debounce: Duration,
    pub document_wait: Duration,
    pub full_wait: Duration,
    pub diagnostics_request: Duration,
    /// Per feature request (hover, definition, ...). Baseline has none.
    pub request: Option<Duration>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            initialize: Duration::from_millis(45_000),
            debounce: Duration::from_millis(150),
            document_wait: Duration::from_millis(5_000),
            full_wait: Duration::from_millis(10_000),
            diagnostics_request: Duration::from_millis(3_000),
            request: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Registration {
    pub identifier: Option<String>,
    pub workspace: bool,
}

#[derive(Default)]
pub(super) struct DiagState {
    pub push: HashMap<String, Vec<Diagnostic>>,
    pub pull: HashMap<String, Vec<Diagnostic>>,
    pub published: HashMap<String, (Instant, Option<i64>)>,
    /// Insertion ordered `textDocument/diagnostic` registrations by id.
    pub registrations: Vec<(String, Registration)>,
}

pub(super) struct Shared {
    pub server_id: String,
    pub state: Mutex<DiagState>,
    pub pushes: broadcast::Sender<String>,
    pub registration: watch::Sender<u64>,
}

struct ServerHandler {
    shared: Arc<Shared>,
    root_uri: String,
    initialization: Option<Value>,
}

impl Handler for ServerHandler {
    fn request(&self, method: &str, params: &Value) -> Option<Value> {
        match method {
            "window/workDoneProgress/create" | "workspace/diagnostic/refresh" => Some(Value::Null),
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let values = items
                    .iter()
                    .map(|i| {
                        configuration_value(
                            self.initialization.as_ref(),
                            i.get("section").and_then(Value::as_str),
                        )
                    })
                    .collect();
                Some(Value::Array(values))
            }
            "client/registerCapability" => {
                self.registrations(params.get("registrations"), true);
                Some(Value::Null)
            }
            "client/unregisterCapability" => {
                self.registrations(params.get("unregisterations"), false);
                Some(Value::Null)
            }
            "workspace/workspaceFolders" => {
                Some(json!([{"name": "workspace", "uri": self.root_uri}]))
            }
            _ => None,
        }
    }

    fn notification(&self, method: &str, params: &Value) {
        if method != "textDocument/publishDiagnostics" {
            return;
        }
        let Some(path) = params
            .get("uri")
            .and_then(Value::as_str)
            .and_then(uri_to_path)
        else {
            return;
        };
        let diagnostics = parse_diagnostics(params.get("diagnostics"));
        let version = params.get("version").and_then(Value::as_i64);
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .published
            .insert(path.clone(), (Instant::now(), version));
        // client.ts:116-121: typescript seeds the cache on first push without waking waiters.
        let seed = self.shared.server_id == "typescript" && !state.push.contains_key(&path);
        state.push.insert(path.clone(), diagnostics);
        drop(state);
        if !seed {
            let _ = self.shared.pushes.send(path);
        }
    }
}

impl ServerHandler {
    fn registrations(&self, list: Option<&Value>, add: bool) {
        let list = list.and_then(Value::as_array).cloned().unwrap_or_default();
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for reg in list
            .iter()
            .filter(|r| r.get("method").and_then(Value::as_str) == Some("textDocument/diagnostic"))
        {
            let id = reg
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            state.registrations.retain(|(k, _)| *k != id);
            if add {
                let opts = reg.get("registerOptions");
                let identifier = opts
                    .and_then(|o| o.get("identifier"))
                    .and_then(Value::as_str)
                    .map(String::from);
                let workspace =
                    opts.and_then(|o| o.get("workspaceDiagnostics")) == Some(&Value::Bool(true));
                state.registrations.push((
                    id,
                    Registration {
                        identifier,
                        workspace,
                    },
                ));
            }
            changed = true;
        }
        drop(state);
        if changed {
            self.shared.registration.send_modify(|v| *v += 1);
        }
    }
}

pub(super) fn parse_diagnostics(value: Option<&Value>) -> Vec<Diagnostic> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| serde_json::from_value(d.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// client.ts:107-114 `configurationValue`.
fn configuration_value(settings: Option<&Value>, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.cloned().unwrap_or(Value::Null);
    };
    let mut current = settings;
    for key in section.split('.') {
        current = current.and_then(|v| v.as_object()).and_then(|o| o.get(key));
    }
    current.cloned().unwrap_or(Value::Null)
}

struct Document {
    version: i64,
    text: String,
}

/// A connected, initialized LSP client.
pub struct Client {
    pub server_id: String,
    pub root: PathBuf,
    directory: PathBuf,
    pub(super) connection: Arc<Connection>,
    pub(super) shared: Arc<Shared>,
    pub(super) timeouts: Timeouts,
    pub(super) sync_kind: Option<i64>,
    pub(super) static_pull: bool,
    files: tokio::sync::Mutex<HashMap<String, Document>>,
    child: tokio::sync::Mutex<Option<Child>>,
    pub(super) background: Mutex<JoinSet<()>>,
}

impl Client {
    /// `LSPClient.create`: wire handlers, run `initialize` (with timeout), send `initialized`
    /// and, when initialization options exist, `workspace/didChangeConfiguration`.
    /// On failure the process is killed and reaped before the error is returned.
    pub async fn create(
        server_id: &str,
        mut handle: Handle,
        root: &Path,
        directory: &Path,
        timeouts: Timeouts,
    ) -> Result<Arc<Self>, IntegrationError> {
        let closed = || IntegrationError::Closed("lsp process stdio unavailable".into());
        let stdout = handle.child.stdout.take().ok_or_else(closed)?;
        let stdin = handle.child.stdin.take().ok_or_else(closed)?;
        let mut background = JoinSet::new();
        if let Some(mut stderr) = handle.child.stderr.take() {
            background.spawn(async move {
                let mut sink = [0u8; 4096];
                while matches!(stderr.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
        let shared = Arc::new(Shared {
            server_id: server_id.to_string(),
            state: Mutex::new(DiagState::default()),
            pushes: broadcast::channel(256).0,
            registration: watch::channel(0).0,
        });
        let root_uri = file_uri(root);
        let init_value = handle.initialization.clone().map(Value::Object);
        let handler = Arc::new(ServerHandler {
            shared: shared.clone(),
            root_uri: root_uri.clone(),
            initialization: init_value.clone(),
        });
        let connection = Connection::start(BufReader::new(stdout), stdin, handler);
        let params = json!({
            "rootUri": root_uri,
            "processId": handle.child.id(),
            "workspaceFolders": [{"name": "workspace", "uri": root_uri}],
            "initializationOptions": init_value.clone().unwrap_or_else(|| json!({})),
            "capabilities": {
                "window": {"workDoneProgress": true},
                "workspace": {
                    "configuration": true,
                    "didChangeWatchedFiles": {"dynamicRegistration": true},
                    "diagnostics": {"refreshSupport": false}
                },
                "textDocument": {
                    "synchronization": {"didOpen": true, "didChange": true},
                    "diagnostic": {"dynamicRegistration": true, "relatedDocumentSupport": true},
                    "publishDiagnostics": {"versionSupport": false}
                }
            }
        });
        let mut client = Self {
            server_id: server_id.to_string(),
            root: root.to_path_buf(),
            directory: directory.to_path_buf(),
            connection,
            shared,
            timeouts,
            sync_kind: None,
            static_pull: false,
            files: tokio::sync::Mutex::new(HashMap::new()),
            child: tokio::sync::Mutex::new(Some(handle.child)),
            background: Mutex::new(background),
        };
        let initialized = client
            .connection
            .request("initialize", params, Some(timeouts.initialize))
            .await;
        let caps = match initialized {
            Ok(v) => v.get("capabilities").cloned().unwrap_or(Value::Null),
            Err(err) => {
                client.shutdown().await;
                return Err(IntegrationError::Failed {
                    name: server_id.to_string(),
                    message: format!("LSPInitializeError: {err}"),
                });
            }
        };
        let sync_kind = match caps.get("textDocumentSync") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(obj) => obj.get("change").and_then(Value::as_i64),
            None => None,
        };
        let static_pull = caps.get("diagnosticProvider").is_some_and(truthy);
        client.sync_kind = sync_kind;
        client.static_pull = static_pull;
        let client = Arc::new(client);
        let handshake = async {
            client.connection.notify("initialized", json!({})).await?;
            if let Some(settings) = init_value {
                client
                    .connection
                    .notify(
                        "workspace/didChangeConfiguration",
                        json!({"settings": settings}),
                    )
                    .await?;
            }
            Ok::<_, IntegrationError>(())
        };
        if let Err(err) = handshake.await {
            client.shutdown().await;
            return Err(err);
        }
        Ok(client)
    }

    pub fn normalize(&self, file: &str) -> String {
        absolute(&self.directory, file)
            .to_string_lossy()
            .into_owned()
    }

    /// `notify.open`: returns the document version that was sent.
    pub async fn open(&self, file: &str) -> Result<i64, IntegrationError> {
        let path = self.normalize(file);
        let text =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| IntegrationError::NotFound {
                    kind: "file",
                    name: format!("{path}: {e}"),
                })?;
        let uri = file_uri(Path::new(&path));
        let mut files = self.files.lock().await;
        if let Some(doc) = files.get(&path) {
            // Diagnostics are not wiped on didChange (clangd only re-emits on real changes).
            self.watched(&uri, FILE_CHANGE_CHANGED).await?;
            let next = doc.version + 1;
            let changes = if self.sync_kind == Some(TEXT_DOCUMENT_SYNC_INCREMENTAL) {
                let (line, character) = end_position(&doc.text);
                json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": line, "character": character}}, "text": text}])
            } else {
                json!([{"text": text}])
            };
            files.insert(
                path,
                Document {
                    version: next,
                    text,
                },
            );
            let params =
                json!({"textDocument": {"uri": uri, "version": next}, "contentChanges": changes});
            self.connection
                .notify("textDocument/didChange", params)
                .await?;
            return Ok(next);
        }
        self.watched(&uri, FILE_CHANGE_CREATED).await?;
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.push.remove(&path);
            state.pull.remove(&path);
        }
        let params = json!({"textDocument": {"uri": uri, "languageId": language_id(&path), "version": 0, "text": text}});
        self.connection
            .notify("textDocument/didOpen", params)
            .await?;
        files.insert(path, Document { version: 0, text });
        Ok(0)
    }

    /// `textDocument/didSave` for an open document (not used by the baseline service flow).
    pub async fn save(&self, file: &str) -> Result<(), IntegrationError> {
        let path = self.normalize(file);
        let files = self.files.lock().await;
        let Some(doc) = files.get(&path) else {
            return Err(IntegrationError::NotFound {
                kind: "open document",
                name: path,
            });
        };
        let params = json!({"textDocument": {"uri": file_uri(Path::new(&path))}, "text": doc.text});
        self.connection.notify("textDocument/didSave", params).await
    }

    /// `textDocument/didClose`: forgets the version so the next open is a fresh didOpen.
    pub async fn close(&self, file: &str) -> Result<(), IntegrationError> {
        let path = self.normalize(file);
        if self.files.lock().await.remove(&path).is_none() {
            return Err(IntegrationError::NotFound {
                kind: "open document",
                name: path,
            });
        }
        self.connection
            .notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": file_uri(Path::new(&path))}}),
            )
            .await
    }

    async fn watched(&self, uri: &str, kind: u8) -> Result<(), IntegrationError> {
        self.connection
            .notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes": [{"uri": uri, "type": kind}]}),
            )
            .await
    }

    /// Feature request (`connection.sendRequest`) with the optional per-request timeout.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, IntegrationError> {
        self.connection
            .request(method, params, self.timeouts.request)
            .await
    }

    /// Raw notification (`connection.sendNotification`).
    pub async fn connection_notify(&self, method: &str, params: Value) {
        if let Err(err) = self.connection.notify(method, params).await {
            tracing::debug!(error = %err, method, "lsp notify failed");
        }
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child
            .try_lock()
            .ok()
            .and_then(|c| c.as_ref().and_then(Child::id))
    }

    /// `shutdown`: end the connection, stop the process and join every owned task. Idempotent.
    pub async fn shutdown(&self) {
        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        self.connection.close().await;
        let mut tasks =
            std::mem::take(&mut *self.background.lock().unwrap_or_else(|e| e.into_inner()));
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
