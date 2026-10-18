//! Purpose: `SessionPrompt.prompt/loop/cancel` and `createUserMessage` (packages/opencode/src/
//! session/prompt.ts:144-156, 594-1079, 1343-1347), `getModel`/`currentModel`/`lastAssistant`,
//! `SessionRevert.cleanup` (revert.ts:101-124) and the agent-not-found error path.
//! Scope: one legacy prompt; the model loop is the `LegacyLoop` port, run under `RunState`.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- futures::FutureExt
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/legacy_prompt.rs
//! Invariants: revert cleanup runs before the user message is written; the user message and its
//! parts are durable before `touch`, the `tools` permission override and the loop. `noReply` returns
//! the user message without running the loop. Agent/model changes publish `session.updated` only when
//! they differ from the stored Session (variant `default` compares as absent).
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use std::sync::Arc;

use futures::FutureExt;
use serde_json::{Value, json};

use super::LegacySession;
use super::parts::Draft;
use super::run_state::Fallback;
use super::store::object;
use super::types::{
    AgentInfo, LegacyError, ModelRef, Object, PromptInput, WithParts, message_id, now_millis,
    str_field, unknown_error,
};

impl LegacySession {
    /// ChildHost seam (3): `resolvePromptParts` then `ops.prompt` on the child, mapped to the
    /// observed reply (task.ts:200-224).
    pub async fn prompt_child(
        &self,
        run: &crate::children::TaskRun,
    ) -> Result<crate::children::ChildReply, LegacyError> {
        let parts = self.resolve_prompt_parts(&run.prompt).await?;
        let result = self
            .prompt(PromptInput {
                session_id: run.child.clone(),
                message_id: Some(message_id()),
                model: Some(ModelRef::new(&run.model.provider_id, &run.model.model_id)),
                agent: Some(run.agent.clone()),
                variant: run.variant.clone(),
                parts,
                ..PromptInput::default()
            })
            .await?;
        Ok(child_reply(&result))
    }

    /// `SessionPrompt.cancel`.
    pub async fn cancel(&self, session: &str) {
        tracing::info!(session.id = session, "cancel");
        self.deps.run_state.cancel(session).await;
    }

    /// `SessionPrompt.prompt`.
    pub async fn prompt(&self, input: PromptInput) -> Result<WithParts, LegacyError> {
        let session = self.deps.store.get(&input.session_id).await?;
        self.cleanup_revert(&session).await?;
        let message = self.create_user_message(&input).await?;
        self.deps.store.touch(&input.session_id).await?;
        let permissions: Vec<Value> = input
            .tools
            .iter()
            .flatten()
            .map(|(tool, enabled)| {
                let action = if enabled.as_bool().unwrap_or(false) {
                    "allow"
                } else {
                    "deny"
                };
                json!({ "permission": tool, "action": action, "pattern": "*" })
            })
            .collect();
        if !permissions.is_empty() {
            self.deps
                .store
                .set_permission(&input.session_id, Value::Array(permissions))
                .await?;
        }
        if input.no_reply == Some(true) {
            return Ok(message);
        }
        self.run(&input.session_id).await
    }

    /// `SessionPrompt.loop`: joins or starts the Session's run.
    pub async fn run(&self, session: &str) -> Result<WithParts, LegacyError> {
        let run_loop = self.deps.run_loop.clone();
        let id = session.to_owned();
        let work = async move { run_loop.run(&id).await }.boxed();
        self.deps
            .run_state
            .ensure_running(session, self.last_assistant_fallback(session), work)
            .await
    }

    pub(super) fn last_assistant_fallback(&self, session: &str) -> Fallback {
        let this = self.clone();
        let session = session.to_owned();
        Arc::new(move || {
            let this = this.clone();
            let session = session.clone();
            async move { this.last_assistant(&session).await }.boxed()
        })
    }

    /// `lastAssistant`: newest non-user message, else the newest message.
    pub async fn last_assistant(&self, session: &str) -> Result<WithParts, LegacyError> {
        if let Some(found) = self
            .deps
            .store
            .find_message(session, |message| !message.is_user())
            .await?
        {
            return Ok(found);
        }
        self.deps
            .store
            .latest(session, 1)
            .await?
            .pop()
            .ok_or_else(|| LegacyError::Unknown("Impossible".into()))
    }

    /// `SessionRevert.cleanup`: drop reverted messages/parts, then clear the revert marker.
    pub async fn cleanup_revert(&self, session: &Object) -> Result<(), LegacyError> {
        let Some(revert) = session.get("revert").and_then(Value::as_object) else {
            return Ok(());
        };
        let id = str_field(session, "id").unwrap_or_default();
        let messages = self.deps.store.messages(id).await?;
        let message_id = str_field(revert, "messageID").unwrap_or_default();
        let part_id = str_field(revert, "partID");
        let index = messages.iter().position(|m| m.id() == message_id);
        if let Some(index) = index {
            let skip = usize::from(part_id.is_some());
            for message in &messages[index + skip..] {
                self.deps.store.remove_message(id, message.id()).await?;
            }
            if let Some(part_id) = part_id {
                let target = &messages[index];
                if let Some(at) = target
                    .parts
                    .iter()
                    .position(|p| str_field(p, "id") == Some(part_id))
                {
                    for part in &target.parts[at..] {
                        let part = str_field(part, "id").unwrap_or_default();
                        self.deps.store.remove_part(id, target.id(), part).await?;
                    }
                }
            }
        }
        self.deps.store.clear_revert(id).await
    }

    /// Agent lookup with the baseline not-found error: publishes `session.error`, then fails.
    pub(super) async fn require_agent(
        &self,
        session: &str,
        name: Option<&str>,
    ) -> Result<AgentInfo, LegacyError> {
        let found = match name {
            Some(name) => self.deps.agents.get(name).await?,
            None => Some(self.deps.agents.default_info().await?),
        };
        if let Some(agent) = found {
            return Ok(agent);
        }
        let available: Vec<String> = self
            .deps
            .agents
            .list()
            .await?
            .into_iter()
            .filter(|agent| agent.hidden != Some(true))
            .map(|agent| agent.name)
            .collect();
        let hint = if available.is_empty() {
            String::new()
        } else {
            format!(" Available agents: {}", available.join(", "))
        };
        let message = format!(
            "Agent not found: \"{}\".{hint}",
            name.unwrap_or("undefined")
        );
        self.deps
            .store
            .publish_error(session, unknown_error(&message))
            .await?;
        Err(LegacyError::Unknown(message))
    }
}

/// Maps a prompt result to `ChildReply`: assistant error message (`data.message`, else `name`),
/// last failed tool error, last text part.
pub fn child_reply(result: &WithParts) -> crate::children::ChildReply {
    let error = result
        .is_assistant()
        .then(|| result.info.get("error"))
        .flatten()
        .filter(|error| !error.is_null())
        .map(|error| {
            error
                .get("data")
                .and_then(|data| data.get("message"))
                .and_then(Value::as_str)
                .or_else(|| error.get("name").and_then(Value::as_str))
                .unwrap_or_default()
                .to_owned()
        });
    let failed_tool = result
        .parts
        .iter()
        .rev()
        .find(|part| str_field(part, "type") == Some("tool") && part["state"]["status"] == "error")
        .and_then(|part| part["state"]["error"].as_str())
        .map(str::to_owned);
    let text = result
        .parts
        .iter()
        .rev()
        .find(|part| str_field(part, "type") == Some("text"))
        .and_then(|part| str_field(part, "text"))
        .map(str::to_owned);
    crate::children::ChildReply {
        error,
        failed_tool,
        text,
    }
}

impl LegacySession {
    /// `currentModel`: Session row model, else the newest user message model, else the default.
    pub async fn current_model(&self, session: &str) -> Result<ModelRef, LegacyError> {
        if let Some(model) = self.deps.store.session_model(session).await? {
            let variant = model
                .get("variant")
                .and_then(Value::as_str)
                .filter(|variant| *variant != "default")
                .map(str::to_owned);
            return Ok(ModelRef {
                provider_id: model["providerID"].as_str().unwrap_or_default().into(),
                model_id: model["id"].as_str().unwrap_or_default().into(),
                variant,
            });
        }
        let found = self
            .deps
            .store
            .find_message(session, |m| m.is_user() && m.info.get("model").is_some())
            .await?;
        if let Some(found) = found
            && let Ok(model) = serde_json::from_value::<ModelRef>(found.info["model"].clone())
        {
            return Ok(model);
        }
        self.deps.provider.default_model().await
    }

    /// `getModel`: resolves a model, publishing `session.error` for `ModelNotFoundError`.
    pub async fn get_model(
        &self,
        provider_id: &str,
        model_id: &str,
        session: &str,
    ) -> Result<Object, LegacyError> {
        match self.deps.provider.get_model(provider_id, model_id).await {
            Err(LegacyError::ModelNotFound {
                provider_id,
                model_id,
                suggestions,
            }) => {
                let hint = if suggestions.is_empty() {
                    String::new()
                } else {
                    format!(" Did you mean: {}?", suggestions.join(", "))
                };
                self.deps
                    .store
                    .publish_error(
                        session,
                        unknown_error(&format!("Model not found: {provider_id}/{model_id}.{hint}")),
                    )
                    .await?;
                Err(LegacyError::ModelNotFound {
                    provider_id,
                    model_id,
                    suggestions,
                })
            }
            other => other,
        }
    }

    /// `createUserMessage`.
    pub async fn create_user_message(&self, input: &PromptInput) -> Result<WithParts, LegacyError> {
        let agent = self
            .require_agent(&input.session_id, input.agent.as_deref())
            .await?;
        let model = match (&input.model, &agent.model) {
            (Some(model), _) => model.clone(),
            (None, Some(model)) => model.clone(),
            (None, None) => self.current_model(&input.session_id).await?,
        };
        let variant = match (&input.variant, &agent.variant, &agent.model) {
            (Some(variant), _, _) => Some(variant.clone()),
            (None, Some(variant), Some(own))
                if own.provider_id == model.provider_id && own.model_id == model.model_id =>
            {
                let full = self
                    .deps
                    .provider
                    .get_model(&model.provider_id, &model.model_id)
                    .await;
                match full {
                    Ok(full) => full
                        .get("variants")
                        .and_then(|variants| variants.get(variant))
                        .filter(|value| !value.is_null() && *value != &Value::Bool(false))
                        .map(|_| variant.clone()),
                    Err(LegacyError::ModelNotFound { .. }) => None,
                    Err(error) => return Err(error),
                }
            }
            _ => None,
        };
        let id = input.message_id.clone().unwrap_or_else(message_id);
        let created = now_millis();
        let mut model_json = json!({ "providerID": model.provider_id, "modelID": model.model_id });
        if let Some(variant) = &variant {
            model_json["variant"] = json!(variant);
        }
        let mut info = object(json!({
            "id": id,
            "role": "user",
            "sessionID": input.session_id,
            "time": { "created": created },
            "tools": input.tools,
            "agent": agent.name,
            "model": model_json,
            "system": input.system,
            "format": input.format,
        }));
        info.retain(|_, value| !value.is_null());

        let current = self.deps.store.get(&input.session_id).await?;
        let current_model = current.get("model");
        let current_variant = current_model
            .and_then(|m| m.get("variant"))
            .and_then(Value::as_str)
            .filter(|v| *v != "default");
        let differs = str_field(&current, "agent") != Some(agent.name.as_str())
            || current_model
                .and_then(|m| m.get("providerID"))
                .and_then(Value::as_str)
                != Some(model.provider_id.as_str())
            || current_model
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                != Some(model.model_id.as_str())
            || current_variant != variant.as_deref();
        if differs {
            self.deps
                .store
                .set_agent_model(
                    &input.session_id,
                    &agent.name,
                    json!({
                        "id": model.model_id,
                        "providerID": model.provider_id,
                        "variant": variant.as_deref().unwrap_or("default"),
                    }),
                    created,
                )
                .await?;
        }

        let draft = Draft {
            session: &input.session_id,
            message: &id,
            agent: &agent,
            input_agent: input.agent.as_deref(),
            model: (&model.provider_id, &model.model_id),
        };
        let mut resolved = Vec::new();
        for part in &input.parts {
            resolved.extend(self.resolve_part(&draft, part).await?);
        }
        let resolved = Self::assign_ids(resolved);
        let output = self
            .deps
            .tools
            .trigger(
                "chat.message",
                json!({
                    "sessionID": input.session_id,
                    "agent": input.agent,
                    "model": input.model,
                    "messageID": input.message_id,
                    "variant": input.variant,
                }),
                json!({ "message": info, "parts": resolved }),
            )
            .await?;
        let info = output
            .get("message")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or(info);
        let parts: Vec<Object> = output
            .get("parts")
            .and_then(Value::as_array)
            .map(|parts| parts.iter().filter_map(Value::as_object).cloned().collect())
            .unwrap_or(resolved);
        let parts = self.normalize_images(parts).await?;

        self.deps.store.update_message(&info).await?;
        for part in &parts {
            self.deps.store.update_part(part).await?;
        }
        Ok(WithParts { info, parts })
    }
}
