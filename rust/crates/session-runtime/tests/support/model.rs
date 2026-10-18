//! Purpose: shared fixtures for runner model tests: recording `RouteFactory`, stub route and
//! baseline-shaped `ModelV2.Info` / `SessionV2.Info` builders (session-runner-model.test.ts).
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/session.rs
//! `-- session-runtime/src/runner/model.rs
//! External:
//! |-- futures::stream
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/runner_model.rs
//! `-- session-runtime/tests/runner_model_location.rs
//! Invariants: fixtures are built from baseline-shaped JSON, never from production helpers.
//! `StubRoute` is a fixture: assertions target the captured `RouteSpec` patch.
//! Contract tests: session-runtime/tests/runner_model*.rs
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use opencode_llm_api::{
    LlmError, LlmEventStream, LlmRequest, PreparedRequest, ProviderId, Route, RouteDefaults,
};
use opencode_schema::provider::ModelInfo;
use opencode_schema::session::Info as SessionInfo;
use opencode_session_runtime::runner::model::{RouteFactory, RouteSpec};
use serde_json::{Value, json};

#[derive(Debug)]
pub struct StubRoute(&'static str, RouteDefaults);

impl Route for StubRoute {
    fn id(&self) -> &str {
        self.0
    }
    fn protocol(&self) -> &str {
        self.0
    }
    fn transport(&self) -> &str {
        "stub"
    }
    fn provider(&self) -> Option<&ProviderId> {
        None
    }
    fn defaults(&self) -> &RouteDefaults {
        &self.1
    }
    fn prepare(&self, _: &LlmRequest) -> Result<PreparedRequest, LlmError> {
        Err(LlmError::invalid_request("stub"))
    }
    fn stream(&self, _: LlmRequest) -> LlmEventStream {
        Box::pin(futures::stream::empty())
    }
}

#[derive(Default)]
pub struct Routes(Mutex<Vec<RouteSpec>>);

impl Routes {
    pub fn last(&self) -> RouteSpec {
        self.0.lock().unwrap().last().cloned().expect("route built")
    }
}

impl RouteFactory for Routes {
    fn route(&self, spec: RouteSpec) -> Arc<dyn Route> {
        let id = spec.route.id();
        self.0.lock().unwrap().push(spec);
        Arc::new(StubRoute(id, RouteDefaults::default()))
    }
}

pub fn model_with(api: Value, variants: Value, request: Value) -> ModelInfo {
    let mut api_obj = json!({ "id": "api-test-model" });
    api_obj
        .as_object_mut()
        .unwrap()
        .extend(api.as_object().unwrap().clone());
    serde_json::from_value(json!({
        "id": "test-model",
        "providerID": "test-provider",
        "name": "Test model",
        "api": api_obj,
        "capabilities": { "tools": true, "input": ["text"], "output": ["text"] },
        "request": request,
        "variants": variants,
        "time": { "released": 0 },
        "cost": [],
        "status": "active",
        "enabled": true,
        "limit": { "context": 100, "output": 20 },
    }))
    .unwrap()
}

pub fn default_request() -> Value {
    json!({
        "headers": { "x-test": "header" },
        "body": { "apiKey": "secret", "custom_extension": { "enabled": true } },
    })
}

pub fn model(api: Value) -> ModelInfo {
    model_with(api, json!([]), default_request())
}

pub fn openai() -> Value {
    json!({ "type": "aisdk", "package": "@ai-sdk/openai", "url": "https://openai.example/v1" })
}

pub fn session(id: &str, model: Option<Value>) -> SessionInfo {
    let mut value = json!({
        "id": id,
        "projectID": "global",
        "title": "test",
        "cost": 0,
        "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
        "time": { "created": 0, "updated": 0 },
        "location": { "directory": "/project" },
    });
    if let Some(m) = model {
        value["model"] = m;
    }
    serde_json::from_value(value).unwrap()
}

pub fn variant_session(id: &str, variant: &str) -> SessionInfo {
    session(
        id,
        Some(json!({ "id": "test-model", "providerID": "test-provider", "variant": variant })),
    )
}

pub fn body(spec: &RouteSpec) -> Value {
    Value::Object(spec.body.clone())
}
