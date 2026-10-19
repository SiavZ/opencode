//! Purpose: real V1 provider-turn orchestration over LegacyStore, native LLM routes and tools.
//! Scope: one legacy run, owned and cancelled by LegacySession RunState.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/legacy/llm_history.rs
//! |-- session-runtime/src/legacy/llm_turn.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/store.rs
//! `-- session-runtime/src/summary.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::StreamExt
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy_location.rs
//! Invariants: one explicit stream call per provider turn; history reloads between turns; only
//! V1 message/part projectors persist output and local tools never execute provider-owned calls.
//! Contract tests: runtime/tests/legacy_model.rs

use std::sync::Arc;

use futures::StreamExt;
use opencode_llm_api::{LlmClient, LlmRequest, SystemPart};
use opencode_platform_api::snapshot::Snapshots;
use opencode_schema::permission::{Effect, Rule};
use opencode_session_api::SessionID;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::llm_turn::Turn;
use super::ports::{LegacyAgents, LegacyLoop, LegacyProvider};
use super::store::{LegacyStore, object};
use super::types::{LegacyError, WithParts, message_id, now_millis, str_field};
use crate::runner::model::SessionRunnerModel;
use crate::runner::ports::ToolMaterializer;
use crate::store::SessionStore;
use crate::summary::Summary;

pub struct LlmLegacyLoop {
    pub store: LegacyStore,
    pub models: Arc<dyn SessionRunnerModel>,
    pub llm: Arc<dyn LlmClient>,
    pub tools: Arc<dyn ToolMaterializer>,
    pub snapshots: Arc<dyn Snapshots>,
    pub agents: Arc<dyn LegacyAgents>,
    pub provider: Arc<dyn LegacyProvider>,
    pub system: Vec<SystemPart>,
}

#[async_trait::async_trait]
impl LegacyLoop for LlmLegacyLoop {
    async fn run(&self, session: &str) -> Result<WithParts, LegacyError> {
        let id =
            SessionID::new(session).map_err(|error| LegacyError::Unknown(error.to_string()))?;
        let mut step = 0usize;
        loop {
            let history = self.store.messages(session).await?;
            let user = history
                .iter()
                .rev()
                .find(|message| message.is_user())
                .ok_or_else(|| LegacyError::Unknown("No user message".into()))?;
            let selected = SessionStore::new(self.store.events().database())
                .get(&id)
                .await?
                .ok_or_else(|| LegacyError::NotFound(session.into()))?;
            let model = self
                .models
                .resolve(&selected)
                .await
                .map_err(|error| LegacyError::Unknown(error.to_string()))?;
            let agent_name = str_field(&user.info, "agent").unwrap_or("build");
            let agent = self.agents.get(agent_name).await?.ok_or_else(|| {
                LegacyError::Unknown(format!("Agent not found: \"{agent_name}\""))
            })?;
            let info = self.store.get(session).await?;
            let permissions = agent
                .permission
                .iter()
                .chain(
                    info.get("permission")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_object),
                )
                .map(|rule| Rule {
                    action: str_field(rule, "permission").unwrap_or_default().into(),
                    resource: str_field(rule, "pattern").unwrap_or("*").into(),
                    effect: match str_field(rule, "action") {
                        Some("deny") => Effect::Deny,
                        Some("ask") => Effect::Ask,
                        _ => Effect::Allow,
                    },
                })
                .collect::<Vec<_>>();
            let tools = self.tools.materialize(&permissions);
            let mut request = LlmRequest::new(model.clone());
            request.system = self.system.clone();
            if let Some(system) = str_field(&user.info, "system") {
                request.system.push(SystemPart::text(system));
            }
            request.messages = super::llm_history::messages(&history)?;
            request.tools = tools.definitions();
            if agent.steps.is_some_and(|limit| step as f64 >= limit) {
                request.tools.clear();
                request.system.push(SystemPart::text("You have reached the maximum number of steps. Summarize your work without calling tools."));
            }
            let assistant = message_id();
            let now = now_millis();
            let mut output = object(
                json!({"id":assistant,"role":"assistant","parentID":user.id(),"sessionID":session,"mode":agent_name,"agent":agent_name,"path":{"cwd":info["directory"],"root":info["directory"]},"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"modelID":model.id,"providerID":model.provider,"time":{"created":now}}),
            );
            if let Some(variant) = user
                .info
                .get("model")
                .and_then(|model| model.get("variant"))
            {
                output.insert("variant".into(), variant.clone());
            }
            self.store.update_message(&output).await?;
            let snapshot = self.snapshots.capture().await;
            let pricing = self
                .provider
                .get_model(model.provider.as_str(), model.id.as_str())
                .await?;
            let cancel = CancellationToken::new();
            let _cancel = cancel.clone().drop_guard();
            let mut turn = Turn::new(self.store.clone(), output, snapshot, pricing);
            let mut stream = self.llm.stream(request);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => {
                        if let Err(error) = turn.accept(event, tools.as_ref(), cancel.clone()).await
                        {
                            turn.fail(&error.to_string()).await?;
                            return Err(error);
                        }
                    }
                    Err(error) => {
                        turn.fail(&error.to_string()).await?;
                        return Err(LegacyError::Unknown(error.to_string()));
                    }
                }
            }
            if !turn.terminal {
                turn.fail("Provider stream ended without a terminal finish event")
                    .await?;
                return Err(LegacyError::Unknown(
                    "Provider stream ended without a terminal finish event".into(),
                ));
            }
            let continue_tools = turn.continue_tools();
            turn.finish(self.snapshots.as_ref()).await?;
            Summary {
                store: self.store.clone(),
                snapshots: self.snapshots.clone(),
                snapshot: None,
            }
            .summarize(session, user.id())
            .await?;
            if !continue_tools {
                return self
                    .store
                    .find_message(session, |row| row.id() == assistant)
                    .await?
                    .ok_or_else(|| LegacyError::Unknown("Assistant projection missing".into()));
            }
            step += 1;
        }
    }

    async fn cancel(&self, session: &str) -> Result<(), LegacyError> {
        for row in self
            .store
            .messages(session)
            .await?
            .into_iter()
            .filter(|row| {
                row.is_assistant()
                    && row
                        .info
                        .get("time")
                        .and_then(|time| time.get("completed"))
                        .is_none()
            })
        {
            let mut info = row.info;
            info["time"]["completed"] = json!(now_millis());
            self.store.update_message(&info).await?;
            for mut part in row.parts {
                if part.get("type").and_then(Value::as_str) == Some("tool")
                    && matches!(
                        part["state"]["status"].as_str(),
                        Some("pending" | "running")
                    )
                {
                    let start = part["state"]["time"]["start"]
                        .as_i64()
                        .unwrap_or_else(now_millis);
                    part["state"] = json!({"status":"error","input":part["state"]["input"],"error":"Tool execution aborted","metadata":{"interrupted":true},"time":{"start":start,"end":now_millis()}});
                    self.store.update_part(&part).await?;
                }
            }
        }
        Ok(())
    }
}
