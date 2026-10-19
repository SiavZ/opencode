//! Purpose: persist one genuine legacy provider turn through V1 message/part projectors.
//! Scope: stream blocks, tool settlement, usage, snapshots and terminal errors.
//! Invariants: provider-owned tools are never dispatched locally; completion is durable.
//! Contract tests: runtime/tests/legacy_model.rs

use std::collections::BTreeMap;

use opencode_llm_api::{FinishReason, LlmEvent};
use opencode_platform_api::snapshot::Snapshots;
use opencode_session_api::{MessageID, SessionID};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::policy::get_usage;
use super::store::{LegacyStore, PART_DELTA, object};
use super::types::{LegacyError, Object, now_millis, part_id, str_field};
use crate::runner::ports::{MaterializedTools, SettleInput};

pub(super) struct Turn {
    store: LegacyStore,
    info: Object,
    snapshot: Option<String>,
    pricing: Object,
    blocks: BTreeMap<String, Object>,
    tools: BTreeMap<String, Object>,
    reason: Option<FinishReason>,
    step_finished: bool,
    pub terminal: bool,
}

impl Turn {
    pub fn new(
        store: LegacyStore,
        info: Object,
        snapshot: Option<String>,
        pricing: Object,
    ) -> Self {
        Self {
            store,
            info,
            snapshot,
            pricing,
            blocks: BTreeMap::new(),
            tools: BTreeMap::new(),
            reason: None,
            step_finished: false,
            terminal: false,
        }
    }

    fn part(&self, kind: &str) -> Object {
        object(
            json!({"id":part_id(),"sessionID":self.info["sessionID"],"messageID":self.info["id"],"type":kind}),
        )
    }

    pub fn continue_tools(&self) -> bool {
        self.reason == Some(FinishReason::ToolCalls)
            && self.tools.values().any(|part| {
                part["state"]["status"] == "completed" || part["state"]["status"] == "error"
            })
    }

    pub async fn accept(
        &mut self,
        event: LlmEvent,
        tools: &dyn MaterializedTools,
        cancel: CancellationToken,
    ) -> Result<(), LegacyError> {
        let reasoning = matches!(&event, LlmEvent::ReasoningStart { .. });
        match event {
            LlmEvent::StepStart { .. } => {
                let mut part = self.part("step-start");
                if let Some(snapshot) = &self.snapshot {
                    part.insert("snapshot".into(), json!(snapshot));
                }
                self.store.update_part(&part).await?;
            }
            LlmEvent::TextStart {
                id,
                provider_metadata,
            }
            | LlmEvent::ReasoningStart {
                id,
                provider_metadata,
            } => {
                let mut part = self.part(if reasoning { "reasoning" } else { "text" });
                part.insert("text".into(), json!(""));
                part.insert("time".into(), json!({"start":now_millis()}));
                if let Some(metadata) = provider_metadata {
                    part.insert("metadata".into(), json!(metadata));
                }
                self.store.update_part(&part).await?;
                self.blocks.insert(id, part);
            }
            LlmEvent::TextDelta { id, text, .. } | LlmEvent::ReasoningDelta { id, text, .. } => {
                if let Some(part) = self.blocks.get_mut(&id) {
                    let next = format!("{}{}", part["text"].as_str().unwrap_or_default(), text);
                    part.insert("text".into(), json!(next));
                    self.store.publish(&PART_DELTA,json!({"sessionID":self.info["sessionID"],"messageID":self.info["id"],"partID":part["id"],"field":"text","delta":text})).await?;
                }
            }
            LlmEvent::TextEnd {
                id,
                provider_metadata,
            }
            | LlmEvent::ReasoningEnd {
                id,
                provider_metadata,
            } => {
                if let Some(part) = self.blocks.get_mut(&id) {
                    part["time"]["end"] = json!(now_millis());
                    if let Some(metadata) = provider_metadata {
                        part.insert("metadata".into(), json!(metadata));
                    }
                    self.store.update_part(part).await?;
                }
            }
            LlmEvent::ToolInputStart { id, name, .. } => {
                let mut part = self.part("tool");
                part.insert("callID".into(), json!(id));
                part.insert("tool".into(), json!(name));
                part.insert(
                    "state".into(),
                    json!({"status":"pending","input":{},"raw":""}),
                );
                self.store.update_part(&part).await?;
                self.tools.insert(id, part);
            }
            LlmEvent::ToolInputDelta { id, text, .. } => {
                if let Some(part) = self.tools.get_mut(&id) {
                    let raw = format!(
                        "{}{}",
                        part["state"]["raw"].as_str().unwrap_or_default(),
                        text
                    );
                    part["state"]["raw"] = json!(raw);
                }
            }
            LlmEvent::ToolInputEnd { .. } => {}
            LlmEvent::ToolCall {
                id,
                name,
                input,
                provider_executed,
                provider_metadata,
            } => {
                let mut part = self.tools.remove(&id).unwrap_or_else(|| self.part("tool"));
                part.insert("callID".into(), json!(id));
                part.insert("tool".into(), json!(name));
                part.insert(
                    "state".into(),
                    json!({"status":"running","input":input,"time":{"start":now_millis()}}),
                );
                if let Some(metadata) = provider_metadata {
                    part.insert("metadata".into(), json!(metadata));
                }
                if provider_executed == Some(true) {
                    part.entry("metadata").or_insert_with(|| json!({}))["providerExecuted"] =
                        json!(true);
                }
                self.store.update_part(&part).await?;
                self.tools.insert(id.clone(), part);
                if provider_executed != Some(true) {
                    let session =
                        SessionID::new(str_field(&self.info, "sessionID").unwrap_or_default())
                            .map_err(|error| LegacyError::Unknown(error.to_string()))?;
                    let message = MessageID::new(str_field(&self.info, "id").unwrap_or_default())
                        .map_err(|error| LegacyError::Unknown(error.to_string()))?;
                    let settlement = tools
                        .settle(
                            SettleInput {
                                session_id: session,
                                agent: str_field(&self.info, "agent").unwrap_or("build").into(),
                                assistant_message_id: message,
                                call_id: id.clone(),
                                name,
                                input,
                            },
                            cancel,
                        )
                        .await
                        .map_err(|error| LegacyError::Unknown(error.to_string()))?;
                    self.result(
                        &id,
                        serde_json::to_value(settlement.result)
                            .map_err(|error| LegacyError::Unknown(error.to_string()))?,
                        settlement
                            .output
                            .map(serde_json::to_value)
                            .transpose()
                            .map_err(|error| LegacyError::Unknown(error.to_string()))?,
                    )
                    .await?;
                }
            }
            LlmEvent::ToolResult {
                id, result, output, ..
            } => {
                self.result(&id, json!(result), output.map(|value| json!(value)))
                    .await?
            }
            LlmEvent::ToolError { id, message, .. } => {
                if let Some(part) = self.tools.get_mut(&id) {
                    let start = part["state"]["time"]["start"].clone();
                    part["state"] = json!({"status":"error","input":part["state"]["input"],"error":message,"time":{"start":start,"end":now_millis()}});
                    self.store.update_part(part).await?;
                }
            }
            LlmEvent::StepFinish {
                reason,
                usage,
                provider_metadata,
                ..
            } => {
                self.reason = Some(reason);
                self.step_finished = true;
                let usage = get_usage(
                    self.pricing.get("cost"),
                    &json!(usage.unwrap_or_default()),
                    provider_metadata
                        .as_ref()
                        .map(|value| json!(value))
                        .as_ref(),
                );
                self.info.insert("cost".into(), usage["cost"].clone());
                self.info.insert("tokens".into(), usage["tokens"].clone());
                let mut part = self.part("step-finish");
                part.insert("reason".into(), json!(reason));
                part.insert("cost".into(), usage["cost"].clone());
                part.insert("tokens".into(), usage["tokens"].clone());
                self.store.update_part(&part).await?;
            }
            LlmEvent::Finish {
                reason,
                usage,
                provider_metadata,
            } => {
                if !self.step_finished {
                    let usage = get_usage(
                        self.pricing.get("cost"),
                        &json!(usage.unwrap_or_default()),
                        provider_metadata
                            .as_ref()
                            .map(|value| json!(value))
                            .as_ref(),
                    );
                    self.info.insert("cost".into(), usage["cost"].clone());
                    self.info.insert("tokens".into(), usage["tokens"].clone());
                }
                self.reason = Some(reason);
                self.terminal = true;
                self.info.insert("finish".into(), json!(reason));
            }
            LlmEvent::ProviderError { message, .. } => return Err(LegacyError::Unknown(message)),
        }
        Ok(())
    }

    async fn result(
        &mut self,
        id: &str,
        result: Value,
        output: Option<Value>,
    ) -> Result<(), LegacyError> {
        if let Some(part) = self.tools.get_mut(id) {
            let start = part["state"]["time"]["start"].clone();
            let value = result.get("value").cloned().unwrap_or(result.clone());
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            part["state"] = json!({"status":"completed","input":part["state"]["input"],"output":text,"title":output.as_ref().and_then(|value|value.get("title")).cloned().unwrap_or(json!("")),"metadata":output.as_ref().and_then(|value|value.get("metadata")).cloned().unwrap_or(json!({})),"time":{"start":start,"end":now_millis()}});
            self.store.update_part(part).await?;
        }
        Ok(())
    }

    pub async fn fail(&mut self, message: &str) -> Result<(), LegacyError> {
        self.info.insert(
            "error".into(),
            json!({"name":"UnknownError","data":{"message":message}}),
        );
        self.info["time"]["completed"] = json!(now_millis());
        self.store.update_message(&self.info).await
    }

    pub async fn finish(&mut self, snapshots: &dyn Snapshots) -> Result<(), LegacyError> {
        for part in self.blocks.values_mut() {
            part["time"]["end"] = json!(now_millis());
            self.store.update_part(part).await?;
        }
        if let (Some(before), Some(after)) = (&self.snapshot, snapshots.capture().await) {
            let files = snapshots
                .files(before, &after)
                .await
                .map_err(|error| LegacyError::Unknown(error.to_string()))?;
            if !files.is_empty() {
                let mut part = self.part("patch");
                part.insert("hash".into(), json!(before));
                part.insert("files".into(), json!(files));
                self.store.update_part(&part).await?;
            }
        }
        self.info["time"]["completed"] = json!(now_millis());
        self.store.update_message(&self.info).await
    }
}
