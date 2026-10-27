//! Purpose: route plugin callbacks through active tool authorization and the real HTTP service port.
//! Scope: Location callback registry, with one context per in-flight plugin tool call.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/mod.rs
//! |-- server/src/services/legacy.rs
//! `-- tool-api/src/context.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio::sync::RwLock
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: unknown call IDs cannot ask permissions; callback errors never imply permission grants.
//! Contract tests: runtime/tests/location_integrations.rs

use async_trait::async_trait;
use opencode_integration_adapters::plugins::Callbacks;
use opencode_integration_api::IntegrationError;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::LegacySessions;
use opencode_tool_api::ToolContext;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct LocationCallbacks {
    calls: Mutex<HashMap<String, ToolContext>>,
    client: Arc<dyn LegacySessions>,
    location: RouteContext,
    bootstrapping: AtomicBool,
    config: Mutex<Value>,
}

impl LocationCallbacks {
    pub fn new(client: Arc<dyn LegacySessions>, location: RouteContext) -> Self {
        Self {
            calls: Mutex::new(HashMap::new()),
            client,
            location,
            bootstrapping: AtomicBool::new(true),
            config: Mutex::new(Value::Null),
        }
    }
    pub fn bootstrap_config(&self, config: Value) {
        *self
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = config;
    }
    pub fn ready(&self) {
        self.bootstrapping.store(false, Ordering::Release);
    }
    pub fn enter(&self, id: String, context: ToolContext) {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, context);
    }
    pub fn leave(&self, id: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
    }
    fn context(&self, params: &Value) -> Result<ToolContext, IntegrationError> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(params["callId"].as_str().unwrap_or_default())
            .cloned()
            .ok_or_else(|| {
                IntegrationError::PermissionDenied(
                    "plugin callback has no active tool invocation".into(),
                )
            })
    }
}

#[async_trait]
impl Callbacks for LocationCallbacks {
    async fn request(&self, method: &str, params: Value) -> Result<Value, IntegrationError> {
        match method {
            "ctx.ask" => {
                let context = self.context(&params)?;
                let input = &params["input"];
                let action = input["permission"]
                    .as_str()
                    .or_else(|| input["action"].as_str())
                    .ok_or_else(|| IntegrationError::Validation("permission is required".into()))?;
                let resources = input["patterns"]
                    .as_array()
                    .or_else(|| input["resources"].as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["*".into()]);
                context
                    .authorize(
                        action,
                        resources,
                        input["always"].as_array().map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        }),
                        input["metadata"].as_object().cloned(),
                    )
                    .await
                    .map_err(|error| IntegrationError::PermissionDenied(error.to_string()))?;
                Ok(Value::Null)
            }
            "client.call" => {
                let path = params["path"].as_array().ok_or_else(|| {
                    IntegrationError::Validation("client path is required".into())
                })?;
                let operation = path
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(".");
                let args = params["args"]
                    .as_array()
                    .and_then(|args| args.first())
                    .cloned()
                    .unwrap_or(Value::Null);
                if operation == "config.get" {
                    return Ok(
                        serde_json::json!({"data": self.config.lock().unwrap_or_else(|error| error.into_inner()).clone()}),
                    );
                }
                if self.bootstrapping.load(Ordering::Acquire) {
                    return Err(IntegrationError::Closed(format!(
                        "Location is still initializing; plugin SDK call {operation} cannot recursively acquire its Location"
                    )));
                }
                self.client
                    .handle(
                        &operation,
                        &self.location,
                        args["path"].as_object().unwrap_or(&Map::new()),
                        args["query"].as_object().unwrap_or(&Map::new()),
                        args.get("body").cloned(),
                    )
                    .await
                    .map(|data| serde_json::json!({"data":data}))
                    .map_err(|error| IntegrationError::Failed {
                        name: operation,
                        message: error.to_string(),
                    })
            }
            _ => Err(IntegrationError::NotFound {
                kind: "plugin callback",
                name: method.into(),
            }),
        }
    }
    async fn notification(&self, method: &str, params: Value) {
        if method == "ctx.metadata" {
            if let Ok(context) = self.context(&params) {
                context.progress.update(
                    params["input"]["title"].as_str(),
                    params["input"]["metadata"]
                        .as_object()
                        .unwrap_or(&Map::new()),
                );
            }
            return;
        }
        if method == "bridge.log" {
            tracing::debug!(message = %params["message"], "plugin log");
        }
        if method == "workspace.register" {
            tracing::warn!(
                "plugin workspace adapter callback requires native workspace RPC implementation"
            );
        }
    }
}
