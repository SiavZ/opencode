//! Purpose: Handle server-initiated LSP requests and diagnostic notifications.
//! Scope: Configuration lookup, dynamic diagnostic registrations and pushed diagnostic publication.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/jsonrpc.rs
//! `-- integration-adapters/src/lsp/text.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/lsp/client.rs
//! Invariants: TypeScript's first diagnostic push seeds without waking waiters; registration order is preserved.
//! Contract tests: integration-adapters/tests/lsp_client.rs

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

use super::{Registration, Shared, parse_diagnostics};
use crate::lsp::jsonrpc::Handler;
use crate::lsp::text::uri_to_path;

pub(super) struct ServerHandler {
    pub(super) shared: Arc<Shared>,
    pub(super) root_uri: String,
    pub(super) initialization: Option<Value>,
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
