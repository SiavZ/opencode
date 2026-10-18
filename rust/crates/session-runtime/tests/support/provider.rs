//! Purpose: FIXTURE support for runner turn tests: a deterministic scripted `LlmClient` (records
//! every request, replays scripted event lists, optional start/gate latch, hanging scripts), an
//! echo tool registered in the real opencode-tool-api `ToolRegistry`, a fixed-model resolver, a
//! counting compaction port, and a harness wiring `LlmSessionRunner` into `LocalSessionExecution`
//! through `LocationRunners` over a real migrated temp SQLite database.
//! Scope: test-only. Nothing here talks to a real provider.
//! Uses:
//! rust/crates/
//! |-- context/src/registry.rs
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/agents.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/tests/support/sql.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::stream
//! `-- tokio::sync::{Barrier, watch}
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/runner_turn.rs
//! |-- session-runtime/tests/runner_turn_steps.rs
//! `-- session-runtime/tests/runner_turn_tools.rs
//! Invariants: every `stream` call records exactly one request before any event is produced;
//! scripts are consumed FIFO; an exhausted script list yields an empty stream (baseline `?? []`).
//! Contract tests: session-runtime/tests/runner_turn*.rs
#![allow(dead_code)]

use super::sql;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use opencode_context::registry::Registry;
use opencode_llm_api::{
    ContentPart, FinishReason, LlmClient, LlmError, LlmEvent, LlmEventStream, LlmRequest, Message,
    MessageRole, Model, PreparedRequest, ProviderFailureClassification, ProviderId, Route,
    RouteDefaults,
};
use opencode_permissions::{AssertInput, PermissionError};
use opencode_schema::location;
use opencode_schema::values::AbsolutePath;
use opencode_session_api::operations::SessionService;
use opencode_session_api::{
    Delivery, LocationRef, PromptInput, PromptRequest, SessionError, SessionID, SessionInfo,
    SessionResult,
};
use opencode_session_runtime::agents::Agents;
use opencode_session_runtime::execution::{LocalSessionExecution, LocationRunners};
use opencode_session_runtime::revert::Revert;
use opencode_session_runtime::runner::SessionRunner;
use opencode_session_runtime::runner::model::{ModelError, SessionRunnerModel};
use opencode_session_runtime::runner::ports::{
    Compaction, CompactionInput, NoCompaction, NoSnapshots, RegistryContext, RegistryMaterializer,
};
use opencode_session_runtime::runner::turn::{LlmSessionRunner, RunnerDeps};
use opencode_session_runtime::service::{LocalSessionService, ResolvedProject, SessionLocations};
use opencode_session_runtime::store::SessionStore;
use opencode_tool_api::{
    Authorizer, PassthroughBounder, Registration, Tool, ToolContent, ToolContext, ToolFailure,
    ToolOutput, ToolRegistry,
};
use serde_json::{Value, json};
use tokio::sync::{Barrier, watch};

pub const SESSION: &str = "ses_runner_turn";

/// One-shot latch.
#[derive(Clone)]
pub struct Latch(watch::Sender<bool>);

impl Default for Latch {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl Latch {
    pub fn open(&self) {
        self.0.send_replace(true);
    }
    pub async fn wait(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|open| *open).await;
    }
}

/// One scripted provider turn.
pub enum Script {
    Events(Vec<Result<LlmEvent, LlmError>>),
    /// Emits the events, then never ends (until dropped by cancellation).
    Hang(Vec<LlmEvent>),
}

/// FIXTURE: deterministic scripted provider.
#[derive(Default)]
pub struct ScriptedLlm {
    pub requests: Mutex<Vec<LlmRequest>>,
    pub scripts: Mutex<VecDeque<Script>>,
    /// When set, the next stream signals `started` and waits for `gate` before emitting.
    pub gate: Mutex<Option<(Latch, Latch)>>,
}

impl ScriptedLlm {
    pub fn script(&self, scripts: Vec<Script>) {
        *self.scripts.lock().unwrap() = scripts.into();
        self.requests.lock().unwrap().clear();
    }
    /// Returns `(started, gate)` for the next stream.
    pub fn gate_next(&self) -> (Latch, Latch) {
        let latches = (Latch::default(), Latch::default());
        *self.gate.lock().unwrap() = Some(latches.clone());
        latches
    }
    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl LlmClient for ScriptedLlm {
    fn prepare(&self, _: LlmRequest) -> Result<PreparedRequest, LlmError> {
        Err(LlmError::invalid_request("unused"))
    }

    fn stream(&self, request: LlmRequest) -> LlmEventStream {
        self.requests.lock().unwrap().push(request);
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Script::Events(vec![]));
        let gate = self.gate.lock().unwrap().take();
        let events: LlmEventStream = match script {
            Script::Events(events) => Box::pin(stream::iter(events)),
            Script::Hang(events) => {
                Box::pin(stream::iter(events.into_iter().map(Ok)).chain(stream::pending()))
            }
        };
        let Some((started, gate)) = gate else {
            return events;
        };
        Box::pin(
            stream::once(async move {
                started.open();
                gate.wait().await;
                events
            })
            .flatten(),
        )
    }
}

pub fn step_start() -> LlmEvent {
    LlmEvent::StepStart { index: 0 }
}
pub fn step_finish(reason: FinishReason) -> LlmEvent {
    LlmEvent::StepFinish {
        index: 0,
        reason,
        usage: None,
        provider_metadata: None,
    }
}
pub fn finish(reason: FinishReason) -> LlmEvent {
    LlmEvent::Finish {
        reason,
        usage: None,
        provider_metadata: None,
    }
}
pub fn tool_call(id: &str, text: &str, provider_executed: Option<bool>) -> LlmEvent {
    LlmEvent::ToolCall {
        id: id.into(),
        name: "echo".into(),
        input: json!({ "text": text }),
        provider_executed,
        provider_metadata: None,
    }
}
pub fn provider_error(message: &str, overflow: bool) -> LlmEvent {
    LlmEvent::ProviderError {
        message: message.into(),
        classification: overflow.then_some(ProviderFailureClassification::ContextOverflow),
        retryable: None,
        provider_metadata: None,
    }
}
/// `[stepStart, stepFinish(stop), finish(stop)]`.
pub fn stop() -> Script {
    Script::Events(
        vec![
            step_start(),
            step_finish(FinishReason::Stop),
            finish(FinishReason::Stop),
        ]
        .into_iter()
        .map(Ok)
        .collect(),
    )
}
/// `[stepStart, toolCall..., stepFinish(tool-calls), finish(tool-calls)]`.
pub fn calls(calls: Vec<LlmEvent>) -> Script {
    let mut events = vec![step_start()];
    events.extend(calls);
    events.push(step_finish(FinishReason::ToolCalls));
    events.push(finish(FinishReason::ToolCalls));
    Script::Events(events.into_iter().map(Ok).collect())
}
pub fn events(events: Vec<LlmEvent>) -> Script {
    Script::Events(events.into_iter().map(Ok).collect())
}

/// Texts of `role` messages in a request (`messageTexts`).
pub fn texts(request: &LlmRequest, role: MessageRole) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message: &&Message| message.role == role)
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            ContentPart::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}
pub fn user_texts(request: &LlmRequest) -> Vec<String> {
    texts(request, MessageRole::User)
}

#[derive(Debug)]
struct FakeRoute(RouteDefaults);

impl Route for FakeRoute {
    fn id(&self) -> &str {
        "fake-route"
    }
    fn protocol(&self) -> &str {
        "fake"
    }
    fn transport(&self) -> &str {
        "fixture"
    }
    fn provider(&self) -> Option<&ProviderId> {
        None
    }
    fn defaults(&self) -> &RouteDefaults {
        &self.0
    }
    fn prepare(&self, _: &LlmRequest) -> Result<PreparedRequest, LlmError> {
        Err(LlmError::invalid_request("unused"))
    }
    fn stream(&self, _: LlmRequest) -> LlmEventStream {
        Box::pin(stream::empty())
    }
}

/// FIXTURE: resolves every Session to `fake/fake-model`.
pub struct FixedModel;

#[async_trait]
impl SessionRunnerModel for FixedModel {
    async fn resolve(&self, _: &SessionInfo) -> Result<Model, ModelError> {
        Ok(Model::new(
            "fake-model",
            "fake",
            Arc::new(FakeRoute(RouteDefaults::default())),
        ))
    }
}

/// FIXTURE: echo tool (`session-runner.test.ts` echo). Optional barrier proves concurrency,
/// optional hang blocks until cancelled.
#[derive(Default)]
pub struct Echo {
    pub executions: Mutex<Vec<String>>,
    pub barrier: Mutex<Option<Arc<Barrier>>>,
    pub hang: Mutex<bool>,
}

#[async_trait]
impl Tool for Echo {
    fn description(&self) -> &str {
        "Echo text"
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] })
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let text = input["text"].as_str().unwrap_or_default().to_owned();
        self.executions.lock().unwrap().push(text.clone());
        let barrier = self.barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                .await
                .map_err(|_| ToolFailure::new("tools did not run concurrently"))?;
        }
        if *self.hang.lock().unwrap() {
            context.cancel.cancelled().await;
            return Err(ToolFailure::new("cancelled"));
        }
        Ok(ToolOutput::new(
            json!({ "text": text }),
            vec![ToolContent::text(text)],
        ))
    }
}

struct DenyAll;

#[async_trait]
impl Authorizer for DenyAll {
    async fn assert(&self, _: AssertInput) -> Result<(), PermissionError> {
        unreachable!("echo never authorizes")
    }
}

/// FIXTURE: overflow compaction that "succeeds" without rewriting history, counting calls.
#[derive(Default)]
pub struct CountingCompaction {
    pub overflow_calls: Mutex<usize>,
}

#[async_trait]
impl Compaction for CountingCompaction {
    async fn compact_if_needed(&self, _: &CompactionInput<'_>) -> Result<bool, SessionError> {
        Ok(false)
    }
    async fn compact_after_overflow(&self, _: &CompactionInput<'_>) -> Result<bool, SessionError> {
        *self.overflow_calls.lock().unwrap() += 1;
        Ok(true)
    }
}

struct OneLocation(Arc<LlmSessionRunner>);

impl LocationRunners for OneLocation {
    fn get(&self, _: &LocationRef) -> Arc<dyn SessionRunner> {
        self.0.clone()
    }
}

struct NoLocations;

#[async_trait]
impl SessionLocations for NoLocations {
    async fn resolve_project(&self, _: &AbsolutePath) -> SessionResult<ResolvedProject> {
        Err(SessionError::Validation("fixture".into()))
    }
    fn revert(&self, _: &location::Ref) -> SessionResult<Revert> {
        Err(SessionError::Validation("fixture".into()))
    }
}

/// Real SQLite + real projectors + real admission/inbox + real runner, scripted provider.
pub struct Harness {
    pub h: sql::Harness,
    pub llm: Arc<ScriptedLlm>,
    pub echo: Arc<Echo>,
    pub agents: Agents,
    pub runner: Arc<LlmSessionRunner>,
    pub execution: Arc<LocalSessionExecution>,
    pub service: LocalSessionService,
    _tools: Registration,
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_compaction(Arc::new(NoCompaction)).await
    }

    pub async fn with_compaction(compaction: Arc<dyn Compaction>) -> Self {
        let h = sql::open().await;
        h.seed_session(SESSION).await;
        let llm = Arc::new(ScriptedLlm::default());
        let echo = Arc::new(Echo::default());
        let registry = ToolRegistry::new(Arc::new(PassthroughBounder));
        let tools = registry
            .register(vec![("echo".into(), echo.clone() as Arc<dyn Tool>)])
            .unwrap();
        let agents = Agents::new();
        let runner = Arc::new(LlmSessionRunner::new(RunnerDeps {
            events: h.events.clone(),
            llm: llm.clone(),
            agents: agents.clone(),
            tools: Arc::new(RegistryMaterializer {
                registry,
                authorizer: Arc::new(DenyAll),
            }),
            models: Arc::new(FixedModel),
            context: Arc::new(RegistryContext(Registry::new())),
            compaction,
            snapshots: Arc::new(NoSnapshots),
            location: LocationRef {
                directory: AbsolutePath::new("/project"),
                workspace_id: None,
            },
        }));
        let execution = Arc::new(LocalSessionExecution::new(
            Arc::new(SessionStore::new(h.db.clone())),
            Arc::new(OneLocation(runner.clone())),
        ));
        let service = LocalSessionService::new(
            h.events.clone(),
            execution.clone(),
            Arc::new(NoLocations),
            "test",
        );
        Self {
            h,
            llm,
            echo,
            agents,
            runner,
            execution,
            service,
            _tools: tools,
        }
    }

    pub fn sid(&self) -> SessionID {
        SessionID::new(SESSION).unwrap()
    }

    /// `session.prompt({...})`; `resume: false` admits only, otherwise the execution is woken.
    pub async fn prompt(&self, text: &str, delivery: Delivery, resume: bool) {
        self.service
            .prompt(PromptRequest {
                id: None,
                session_id: self.sid(),
                prompt: PromptInput::text(text),
                delivery: Some(delivery),
                resume: Some(resume),
            })
            .await
            .unwrap();
    }

    pub async fn resume(&self) -> SessionResult<()> {
        self.service.resume(&self.sid()).await
    }

    /// Projected context as JSON values (`session.context(sessionID)`).
    pub async fn context(&self) -> Vec<Value> {
        self.service
            .context(&self.sid())
            .await
            .unwrap()
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect()
    }

    pub async fn types(&self) -> Vec<String> {
        self.context()
            .await
            .iter()
            .map(|m| m["type"].as_str().unwrap().to_owned())
            .collect()
    }

    pub fn executions(&self) -> Vec<String> {
        self.echo.executions.lock().unwrap().clone()
    }

    pub fn set_steps(&self, steps: i64) -> opencode_session_runtime::agents::Registration {
        self.agents.transform(move |draft| {
            draft.update(opencode_schema::agent::ID::new("build"), |agent| {
                agent.steps = opencode_schema::values::PositiveInt::new(steps);
            })
        })
    }
}
