//! Purpose: location-bound legacy session HTTP operations using actual prompt and snapshot services.
//! Scope: resolver contract and adapter dispatch, async prompts are owned by the adapter tracker.
//! Uses:
//! rust/crates/
//! |-- runtime/src/legacy.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/revert.rs
//! `-- session-runtime/src/summary.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/src/legacy.rs
//! Invariants: no V2 transcript translation; all operations run the real legacy location services.
//! Contract tests: runtime/tests/legacy_operations.rs

use std::sync::Arc;

use opencode_platform_api::snapshot::Snapshots;
use opencode_session_runtime::legacy::LegacySession;
use opencode_session_runtime::legacy::revert::LegacyRevert;
use opencode_session_runtime::legacy::types::{CommandInput, PromptInput, ShellInput};
use opencode_session_runtime::summary::Summary;
use serde_json::{Map, Value, json};

use super::{LegacyAdapter, LegacyError, RouteContext, internal, map_error, numbers};

#[derive(Clone)]
pub struct LegacyLocationServices {
    pub session: Arc<LegacySession>,
    pub snapshots: Arc<dyn Snapshots>,
    pub guard: Arc<dyn Send + Sync>,
    pub root: std::path::PathBuf,
}

#[async_trait::async_trait]
pub trait LegacyLocations: Send + Sync {
    async fn get(&self, context: &RouteContext) -> Result<LegacyLocationServices, LegacyError>;
    async fn shutdown(&self) {}
    async fn background(&self, _session: &str) -> Result<bool, LegacyError> {
        Err(LegacyError::Unavailable(
            "legacy background registry".into(),
        ))
    }
    async fn cancel_children(&self, _session: &str) {}
}

impl LegacyAdapter {
    pub(super) async fn located(
        &self,
        operation: &str,
        context: &RouteContext,
        id: &str,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        if operation != "session.diff" {
            self.store.get(id).await.map_err(map_error)?;
        }
        let services = self
            .locations
            .as_ref()
            .ok_or_else(|| LegacyError::Unavailable("legacy location services".into()))?
            .get(context)
            .await?;
        let revert = LegacyRevert {
            store: self.store.clone(),
            snapshots: services.snapshots.clone(),
            run_state: self.run_state.clone(),
            root: services.root.clone(),
        };
        let mut input = payload.unwrap_or_else(|| json!({}));
        input
            .as_object_mut()
            .ok_or_else(|| LegacyError::BadRequest("Invalid session payload".into()))?
            .insert("sessionID".into(), json!(id));
        match operation {
            "session.summarize" => {
                let info = self.store.get(id).await.map_err(map_error)?;
                services
                    .session
                    .cleanup_revert(&info)
                    .await
                    .map_err(map_error)?;
                let agent = self
                    .store
                    .messages(id)
                    .await
                    .map_err(map_error)?
                    .into_iter()
                    .rev()
                    .find(|message| message.is_user())
                    .and_then(|message| {
                        message
                            .info
                            .get("agent")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "build".into());
                opencode_session_runtime::legacy::compaction::LegacyCompaction {
                    store: &self.store,
                }
                .create(
                    id,
                    &agent,
                    json!({"providerID":input["providerID"],"modelID":input["modelID"]}),
                    input["auto"].as_bool().unwrap_or(false),
                    None,
                )
                .await
                .map_err(map_error)?;
                services.session.run(id).await.map_err(map_error)?;
                Ok(json!(true))
            }
            "session.prompt" | "session.prompt_async" => {
                let input: PromptInput = serde_json::from_value(input)
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
                if operation == "session.prompt_async" {
                    let session = services.session;
                    let guard = services.guard;
                    let stop = self.stop.clone();
                    self.tasks.spawn(async move {
                        let _guard = guard;
                        let id = input.session_id.clone();
                        let result = tokio::select! {
                            biased;
                            _ = stop.cancelled() => { session.cancel(&id).await; return; },
                            result = session.prompt(input) => result,
                        };
                        if let Err(error) = result {
                            let _ = session
                                .store()
                                .publish_error(
                                    &id,
                                    opencode_session_runtime::legacy::types::unknown_error(
                                        &error.to_string(),
                                    ),
                                )
                                .await;
                        }
                    });
                    return Ok(Value::Null);
                }
                services
                    .session
                    .prompt(input)
                    .await
                    .map_err(map_error)
                    .and_then(|value| serde_json::to_value(value).map(numbers).map_err(internal))
            }
            "session.command" => {
                let input: CommandInput = serde_json::from_value(input)
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
                services
                    .session
                    .command(input)
                    .await
                    .map_err(map_error)
                    .and_then(|value| serde_json::to_value(value).map(numbers).map_err(internal))
            }
            "session.shell" => {
                let input: ShellInput = serde_json::from_value(input)
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
                services
                    .session
                    .shell(input)
                    .await
                    .map_err(map_error)
                    .and_then(|value| serde_json::to_value(value).map(numbers).map_err(internal))
            }
            "session.init" => {
                services
                    .session
                    .command(CommandInput {
                        session_id: id.into(),
                        message_id: input["messageID"].as_str().map(str::to_owned),
                        model: Some(format!(
                            "{}/{}",
                            input["providerID"].as_str().unwrap_or_default(),
                            input["modelID"].as_str().unwrap_or_default()
                        )),
                        command: "init".into(),
                        arguments: String::new(),
                        ..CommandInput::default()
                    })
                    .await
                    .map_err(|error| LegacyError::BadRequest(error.to_string()))?;
                Ok(json!(true))
            }
            "session.share" => {
                services.session.share(id).await.map_err(map_error)?;
                self.store
                    .get(id)
                    .await
                    .map(Value::Object)
                    .map(numbers)
                    .map_err(map_error)
            }
            "session.unshare" => {
                services.session.unshare(id).await.map_err(map_error)?;
                self.store
                    .get(id)
                    .await
                    .map(Value::Object)
                    .map(numbers)
                    .map_err(map_error)
            }
            "session.revert" => revert
                .revert(
                    id,
                    input["messageID"]
                        .as_str()
                        .ok_or_else(|| LegacyError::BadRequest("Missing messageID".into()))?,
                    input["partID"].as_str(),
                )
                .await
                .map(Value::Object)
                .map(numbers)
                .map_err(map_error),
            "session.unrevert" => revert
                .unrevert(id)
                .await
                .map(Value::Object)
                .map(numbers)
                .map_err(map_error),
            "session.diff" => Summary {
                store: self.store.clone(),
                snapshots: services.snapshots,
                snapshot: None,
            }
            .diff(id, query.get("messageID").and_then(Value::as_str))
            .await
            .map(Value::Array)
            .map_err(map_error),
            _ => Err(LegacyError::Unavailable(operation.into())),
        }
    }
}
