//! Purpose: compose the real legacy task tool with existing child orchestration and provider loop.
//! Scope: ChildOps adapter and immutable task-aware materialization, not a second execution runner.
//! Invariants: child prompts use the same LegacySession and RunState; weak backreferences avoid cycles.
//! Contract tests: runtime/tests/legacy_model.rs

use std::sync::{Arc, OnceLock, Weak};

use opencode_llm_api::{ToolDefinition, ToolResultValue};
use opencode_permissions::{AssertInput, PermissionService};
use opencode_schema::permission::{Effect, Rule};
use opencode_session_runtime::children::{
    AgentInfo, ChildError, ChildOps, ChildReply, Children, ModelSel, TaskInput, TaskRun,
    TaskSettings,
};
use opencode_session_runtime::legacy::LegacySession;
use opencode_session_runtime::legacy::llm_loop::LlmLegacyLoop;
use opencode_session_runtime::legacy::ports::{LegacyAgents, LegacyLoop};
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_session_runtime::legacy::types::{LegacyError, PromptInput, WithParts};
use opencode_session_runtime::runner::ports::{
    MaterializedTools, RegistryMaterializer, SettleError, SettleInput, Settlement, ToolMaterializer,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{Lease, adapters};

pub struct Ops {
    pub session: Arc<OnceLock<Weak<LegacySession>>>,
    pub agents: Arc<adapters::Agents>,
    pub permissions: Arc<PermissionService>,
    pub store: LegacyStore,
}

impl Ops {
    fn session(&self) -> Result<Arc<LegacySession>, ChildError> {
        self.session
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ChildError::Failed("Legacy session scope closed".into()))
    }
}

#[async_trait::async_trait]
impl ChildOps for Ops {
    fn scope(&self) -> Option<Arc<dyn Send + Sync>> {
        self.session()
            .ok()
            .map(|session| session as Arc<dyn Send + Sync>)
    }
    async fn agent(&self, name: &str) -> Result<Option<AgentInfo>, ChildError> {
        Ok(self
            .agents
            .get(name)
            .await
            .map_err(child_error)?
            .map(|agent| AgentInfo {
                name: agent.name,
                permission: agent
                    .permission
                    .into_iter()
                    .filter_map(|rule| serde_json::from_value(json!(rule)).ok())
                    .collect(),
                model: agent.model.map(|model| ModelSel {
                    model_id: model.model_id,
                    provider_id: model.provider_id,
                }),
            }))
    }
    async fn ask_permission(
        &self,
        input: &TaskInput,
        permission: &str,
        pattern: &str,
    ) -> Result<(), ChildError> {
        let mut request =
            AssertInput::new(input.session_id.clone(), permission, vec![pattern.into()]);
        request.agent = Some(input.agent.clone());
        request.save = Some(vec!["*".into()]);
        request.metadata = Some(Map::from_iter([
            ("description".into(), json!(input.description)),
            ("subagent_type".into(), json!(input.subagent_type)),
        ]));
        self.permissions
            .assert(request)
            .await
            .map_err(|error| ChildError::PermissionDenied(error.to_string()))
    }
    async fn tool_metadata(&self, input: &TaskInput, title: &str, metadata: &Map<String, Value>) {
        if let Ok(Some(message)) = self
            .store
            .find_message(&input.session_id, |row| row.id() == input.message_id)
            .await
            && let Some(mut part) = message.parts.into_iter().rev().find(|part| {
                part["type"] == "tool"
                    && part["tool"] == "task"
                    && part["state"]["status"] == "running"
            })
        {
            part["state"]["title"] = json!(title);
            part["state"]["metadata"] = json!(metadata);
            let _ = self.store.update_part(&part).await;
        }
    }
    fn prompt_ready(&self) -> Result<(), ChildError> {
        self.session().map(|_| ())
    }
    async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError> {
        self.session()?.prompt_child(run).await.map_err(child_error)
    }
    async fn cancel(&self, id: &str) {
        if let Ok(session) = self.session() {
            session.cancel(id).await;
        }
    }
    async fn prompt_parent(
        &self,
        id: &str,
        agent: &str,
        variant: Option<String>,
        text: String,
    ) -> Result<(), ChildError> {
        self.session()?
            .prompt(PromptInput {
                session_id: id.into(),
                agent: Some(agent.into()),
                variant,
                parts: vec![
                    serde_json::from_value(json!({"type":"text","text":text,"synthetic":true}))
                        .map_err(|error| ChildError::Failed(error.to_string()))?,
                ],
                ..PromptInput::default()
            })
            .await
            .map(|_| ())
            .map_err(child_error)
    }
}

fn child_error(error: LegacyError) -> ChildError {
    match error {
        LegacyError::Cancelled => ChildError::Cancelled,
        other => ChildError::Failed(other.to_string()),
    }
}

pub struct Loop {
    pub inner: LlmLegacyLoop,
    pub lease: Arc<Lease>,
    pub children: Children,
}

#[async_trait::async_trait]
impl LegacyLoop for Loop {
    async fn run(&self, id: &str) -> Result<WithParts, LegacyError> {
        let _lease = &self.lease;
        self.inner.run(id).await
    }
    async fn cancel(&self, id: &str) -> Result<(), LegacyError> {
        self.children.cancel_tree(id).await;
        self.inner.cancel(id).await
    }
}

pub fn settings(config: &Value) -> TaskSettings {
    TaskSettings {
        subagent_depth: config
            .get("subagent_depth")
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        primary_tools: config
            .get("experimental")
            .and_then(|value| value.get("primary_tools"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        background_subagents: config
            .get("experimental")
            .and_then(|value| value.get("backgroundSubagents"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

pub struct Materializer {
    pub base: Arc<RegistryMaterializer>,
    pub children: Children,
    pub settings: TaskSettings,
}

impl ToolMaterializer for Materializer {
    fn materialize(&self, permissions: &[Rule]) -> Arc<dyn MaterializedTools> {
        let rules = permissions
            .iter()
            .map(|rule| {
                opencode_permissions::Rule::new(
                    rule.action.clone(),
                    rule.resource.clone(),
                    match rule.effect {
                        Effect::Allow => opencode_permissions::Effect::Allow,
                        Effect::Ask => opencode_permissions::Effect::Ask,
                        Effect::Deny => opencode_permissions::Effect::Deny,
                    },
                )
            })
            .collect::<Vec<_>>();
        Arc::new(Tools {
            base: self.base.materialize(permissions),
            children: self.children.clone(),
            settings: self.settings.clone(),
            advertise: !opencode_permissions::wholly_disabled("task", &rules),
        })
    }
}

struct Tools {
    base: Arc<dyn MaterializedTools>,
    children: Children,
    settings: TaskSettings,
    advertise: bool,
}

#[async_trait::async_trait]
impl MaterializedTools for Tools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.base.definitions();
        if self.advertise {
            definitions.push(ToolDefinition {name:"task".into(),description:"Launch a specialized subagent to perform a task.".into(),input_schema:serde_json::from_value(json!({"type":"object","properties":{"description":{"type":"string"},"prompt":{"type":"string"},"subagent_type":{"type":"string"},"task_id":{"type":"string"},"command":{"type":"string"},"background":{"type":"boolean"}},"required":["description","prompt","subagent_type"],"additionalProperties":false})).expect("literal object"),output_schema:None,cache:None,metadata:None,native:None});
        }
        definitions
    }
    async fn settle(
        &self,
        input: SettleInput,
        cancel: CancellationToken,
    ) -> Result<Settlement, SettleError> {
        if input.name != "task" {
            return self.base.settle(input, cancel).await;
        }
        if !self.advertise {
            return Err(SettleError::Declined("Task tool is disabled".into()));
        }
        let text = |key: &str| {
            input
                .input
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let Some(description) = text("description") else {
            return Ok(failure("Invalid tool input: missing description"));
        };
        let Some(prompt) = text("prompt") else {
            return Ok(failure("Invalid tool input: missing prompt"));
        };
        let Some(subagent_type) = text("subagent_type") else {
            return Ok(failure("Invalid tool input: missing subagent_type"));
        };
        let output = self
            .children
            .run(
                &self.settings,
                TaskInput {
                    session_id: input.session_id.as_str().into(),
                    message_id: input.assistant_message_id.as_str().into(),
                    agent: input.agent,
                    description,
                    prompt,
                    subagent_type,
                    task_id: text("task_id"),
                    command: text("command"),
                    background: input.input.get("background").and_then(Value::as_bool),
                    bypass_agent_check: false,
                },
                cancel,
            )
            .await;
        match output {
            Ok(output) => Ok(Settlement {
                result: ToolResultValue::Text(json!(output.output)),
                output: Some(opencode_llm_api::ToolOutput {
                    structured: json!({"title":output.title,"metadata":output.metadata}),
                    content: vec![opencode_llm_api::ToolContent::Text {
                        text: output.output,
                    }],
                }),
                output_paths: Vec::new(),
            }),
            Err(ChildError::PermissionDenied(error)) => Err(SettleError::Declined(error)),
            Err(error) => Ok(failure(&error.to_string())),
        }
    }
}

fn failure(error: &str) -> Settlement {
    Settlement {
        result: ToolResultValue::Error(json!(error)),
        output: None,
        output_paths: Vec::new(),
    }
}
