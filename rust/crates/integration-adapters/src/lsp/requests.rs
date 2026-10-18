//! Purpose: `LanguageService` trait implementation for `LspService`: status, hasClients,
//! touchFile with wait modes, merged diagnostics and the feature requests with the baseline
//! error swallowing (`.catch(() => null | [])`), flattening and workspace-symbol kind filter.
//! Ports packages/opencode/src/lsp/lsp.ts:264-485.
//! Scope: request fan-out across the clients of one file. Lifecycle lives in service.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/service.rs
//! |-- integration-adapters/src/lsp/text.rs
//! `-- integration-api/src/language.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- integration-adapters/tests/lsp_service.rs
//! Invariants: per-client request failures never fail the call, null results are dropped after
//! flattening, workspace symbols keep the 8 baseline kinds and at most 10 per client, touchFile
//! never returns an error for server-side failures (baseline `.catch(() => {})`).
//! Contract tests: integration-adapters/tests/lsp_service.rs

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use opencode_integration_api::IntegrationError;
use opencode_integration_api::language::{
    AnySymbol, ConnectionState, Diagnostic, DiagnosticsWait, LanguageService, LocInput, Status,
    Symbol,
};
use serde_json::{Value, json};

use super::client::Client;
use super::service::LspService;
use super::text::{absolute, file_uri, relative, uri_to_path};

/// lsp.ts:85-94: Class, Function, Method, Interface, Variable, Constant, Struct, Enum.
pub const WORKSPACE_SYMBOL_KINDS: [u32; 8] = [5, 12, 6, 11, 13, 14, 23, 10];

fn position_params(service: &LspService, input: &LocInput) -> Value {
    let path = absolute(&service.ctx.directory, &input.file);
    json!({
        "textDocument": {"uri": file_uri(&path)},
        "position": {"line": input.line, "character": input.character}
    })
}

/// `results.flat().filter(Boolean)`.
fn flatten(results: Vec<Value>) -> Vec<Value> {
    results
        .into_iter()
        .flat_map(|v| match v {
            Value::Array(items) => items,
            other => vec![other],
        })
        .filter(truthy)
        .collect()
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Bool(false))
        && v.as_str() != Some("")
        && v.as_f64() != Some(0.0)
}

impl LspService {
    async fn run(
        &self,
        file: &str,
        method: &'static str,
        params: Value,
        fallback: Value,
    ) -> Vec<Value> {
        let clients = self.clients_for(file).await;
        let calls = clients.iter().map(|c| {
            let params = params.clone();
            let fallback = fallback.clone();
            async move { c.request(method, params).await.unwrap_or(fallback) }
        });
        futures::future::join_all(calls).await
    }

    async fn call_hierarchy(&self, input: LocInput, direction: &'static str) -> Vec<Value> {
        let params = position_params(self, &input);
        let clients = self.clients_for(&input.file).await;
        let calls = clients.iter().map(|c| {
            let params = params.clone();
            async move {
                let items = c
                    .request("textDocument/prepareCallHierarchy", params)
                    .await
                    .unwrap_or(json!([]));
                let Some(first) = items.as_array().and_then(|a| a.first()).cloned() else {
                    return json!([]);
                };
                c.request(direction, json!({"item": first}))
                    .await
                    .unwrap_or(json!([]))
            }
        });
        flatten(futures::future::join_all(calls).await)
    }
}

async fn touch_one(
    client: Arc<Client>,
    file: String,
    wait: Option<DiagnosticsWait>,
) -> Result<(), IntegrationError> {
    let after = Instant::now();
    let version = client.open(&file).await?;
    if let Some(mode) = wait {
        client
            .wait_for_diagnostics(&file, version, mode, after)
            .await;
    }
    Ok(())
}

#[async_trait]
impl LanguageService for LspService {
    async fn init(&self) -> Result<(), IntegrationError> {
        Ok(())
    }

    async fn status(&self) -> Vec<Status> {
        self.clients()
            .iter()
            .map(|c| Status {
                id: c.server_id.clone(),
                name: c.server_id.clone(),
                root: relative(&self.ctx.directory, &c.root),
                status: ConnectionState::Connected,
            })
            .collect()
    }

    async fn has_clients(&self, file: &str) -> bool {
        self.has_clients_for(file)
    }

    async fn touch_file(
        &self,
        file: &str,
        wait: Option<DiagnosticsWait>,
    ) -> Result<(), IntegrationError> {
        tracing::info!(file, "touching file");
        let clients = self.clients_for(file).await;
        let touches = clients
            .into_iter()
            .map(|c| touch_one(c, file.to_string(), wait));
        if let Some(Err(err)) = futures::future::join_all(touches)
            .await
            .into_iter()
            .find(Result::is_err)
        {
            tracing::debug!(error = %err, "touchFile failed");
        }
        Ok(())
    }

    async fn diagnostics(&self) -> BTreeMap<String, Vec<Diagnostic>> {
        let mut out: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();
        for client in self.clients() {
            for (path, items) in client.diagnostics() {
                out.entry(path).or_default().extend(items);
            }
        }
        out
    }

    async fn hover(&self, input: LocInput) -> Result<Value, IntegrationError> {
        let params = position_params(self, &input);
        Ok(Value::Array(
            self.run(&input.file, "textDocument/hover", params, Value::Null)
                .await,
        ))
    }

    async fn definition(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError> {
        let params = position_params(self, &input);
        Ok(flatten(
            self.run(&input.file, "textDocument/definition", params, Value::Null)
                .await,
        ))
    }

    async fn references(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError> {
        let mut params = position_params(self, &input);
        params["context"] = json!({"includeDeclaration": true});
        Ok(flatten(
            self.run(&input.file, "textDocument/references", params, json!([]))
                .await,
        ))
    }

    async fn implementation(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError> {
        let params = position_params(self, &input);
        Ok(flatten(
            self.run(
                &input.file,
                "textDocument/implementation",
                params,
                Value::Null,
            )
            .await,
        ))
    }

    async fn document_symbol(&self, uri: &str) -> Result<Vec<AnySymbol>, IntegrationError> {
        let file = uri_to_path(uri)
            .ok_or_else(|| IntegrationError::Validation(format!("not a file uri: {uri}")))?;
        let params = json!({"textDocument": {"uri": uri}});
        let results = self
            .run(&file, "textDocument/documentSymbol", params, json!([]))
            .await;
        Ok(flatten(results)
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect())
    }

    async fn workspace_symbol(&self, query: &str) -> Result<Vec<Symbol>, IntegrationError> {
        let calls = self.clients().into_iter().map(|c| async move {
            let result = c.request("workspace/symbol", json!({"query": query})).await;
            let Ok(Value::Array(items)) = result else {
                return Vec::new();
            };
            items
                .into_iter()
                .filter_map(|v| serde_json::from_value::<Symbol>(v).ok())
                .filter(|s| WORKSPACE_SYMBOL_KINDS.contains(&s.kind))
                .take(10)
                .collect::<Vec<_>>()
        });
        Ok(futures::future::join_all(calls)
            .await
            .into_iter()
            .flatten()
            .collect())
    }

    async fn prepare_call_hierarchy(
        &self,
        input: LocInput,
    ) -> Result<Vec<Value>, IntegrationError> {
        let params = position_params(self, &input);
        Ok(flatten(
            self.run(
                &input.file,
                "textDocument/prepareCallHierarchy",
                params,
                json!([]),
            )
            .await,
        ))
    }

    async fn incoming_calls(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError> {
        Ok(self
            .call_hierarchy(input, "callHierarchy/incomingCalls")
            .await)
    }

    async fn outgoing_calls(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError> {
        Ok(self
            .call_hierarchy(input, "callHierarchy/outgoingCalls")
            .await)
    }

    async fn shutdown(&self) {
        self.shutdown_all().await;
    }
}
