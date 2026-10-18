//! Purpose: legacy Session title generation (`SessionPrompt.ensureTitle`, packages/opencode/src/
//! session/prompt.ts:193-253): eligibility (root Session, default title, exactly one real user
//! message), subtask-only shortcut, the `title` agent request, `<think>` stripping, first non-empty
//! line, 100-char truncation and `Session.setTitle`.
//! Scope: one Session; the provider turn is the `TitleModel` port yielding an `opencode_llm_api`
//! event stream (legacy `toModelMessages`, small-model selection and `LLM.stream` live upstream).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/template.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- async_trait::async_trait
//! `-- futures::StreamExt
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/tests/title.rs
//! Invariants: no provider call unless the Session is eligible and a `title` agent exists. Stream
//! failures propagate (baseline `Effect.orDie`); a `setTitle` failure is logged and swallowed.
//! Contract tests: session-runtime/tests/title.rs

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use opencode_llm_api::{LlmEvent, LlmEventStream};

use crate::legacy::ports::LegacyAgents;
use crate::legacy::store::LegacyStore;
use crate::legacy::template::is_default_title;
use crate::legacy::types::{AgentInfo, LegacyError, Object, WithParts, part_type, str_field};

/// What the title request should contain after the fixed instruction message.
#[derive(Clone, Debug, PartialEq)]
pub enum TitleContext {
    /// Only subtask parts: one user message with the subtask prompts joined by newlines.
    Subtasks(String),
    /// History up to and including the first real user message (`toModelMessages(context, model)`).
    History(Vec<WithParts>),
}

/// One title request (`llm.stream({ agent, user, system: [], small: true, tools: {}, retries: 2 })`).
#[derive(Clone, Debug, PartialEq)]
pub struct TitleRequest {
    pub session_id: String,
    pub agent: AgentInfo,
    /// The first real user message info.
    pub user: Object,
    pub provider_id: String,
    pub model_id: String,
    /// Prefix message: `"Generate a title for this conversation:\n"`.
    pub instruction: &'static str,
    pub context: TitleContext,
}

pub const TITLE_INSTRUCTION: &str = "Generate a title for this conversation:\n";

/// Runs the title provider turn. Implemented by the runtime over the legacy LLM adapter.
#[async_trait]
pub trait TitleModel: Send + Sync {
    async fn stream(&self, request: TitleRequest) -> Result<LlmEventStream, LegacyError>;
}

/// No legacy title model composed yet.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnwiredTitleModel;

#[async_trait]
impl TitleModel for UnwiredTitleModel {
    async fn stream(&self, _: TitleRequest) -> Result<LlmEventStream, LegacyError> {
        Err(LegacyError::NotYetPorted(
            "legacy LLM.stream + MessageV2.toModelMessages for title generation",
        ))
    }
}

fn is_real_user(message: &WithParts) -> bool {
    message.is_user()
        && !message
            .parts
            .iter()
            .all(|part| part.get("synthetic").and_then(serde_json::Value::as_bool) == Some(true))
}

/// Cleans raw model output: strip `<think>..</think>` blocks, first non-empty trimmed line, then
/// truncate to 100 chars (97 + "...").
pub fn clean_title(text: &str) -> Option<String> {
    let mut stripped = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        let Some(end) = rest[start..].find("</think>") else {
            break;
        };
        stripped.push_str(&rest[..start]);
        let after = &rest[start + end + "</think>".len()..];
        rest = after.trim_start();
    }
    stripped.push_str(rest);
    let line = stripped
        .split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let units: Vec<u16> = line.encode_utf16().collect();
    if units.len() > 100 {
        return Some(format!("{}...", String::from_utf16_lossy(&units[..97])));
    }
    Some(line.to_owned())
}

/// `ensureTitle` collaborators.
#[derive(Clone)]
pub struct Titles {
    pub store: LegacyStore,
    pub agents: Arc<dyn LegacyAgents>,
    pub model: Arc<dyn TitleModel>,
}

impl Titles {
    /// `ensureTitle({ session, history, providerID, modelID })`. Returns the title it set.
    pub async fn ensure(
        &self,
        session: &Object,
        history: &[WithParts],
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<String>, LegacyError> {
        if session
            .get("parentID")
            .is_some_and(|parent| !parent.is_null())
        {
            return Ok(None);
        }
        if !is_default_title(str_field(session, "title").unwrap_or_default()) {
            return Ok(None);
        }
        let Some(index) = history.iter().position(is_real_user) else {
            return Ok(None);
        };
        if history.iter().filter(|m| is_real_user(m)).count() != 1 {
            return Ok(None);
        }
        let first = &history[index];
        let subtasks: Vec<&str> = first
            .parts
            .iter()
            .filter(|part| part_type(part) == "subtask")
            .filter_map(|part| str_field(part, "prompt"))
            .collect();
        let only_subtasks =
            !subtasks.is_empty() && first.parts.iter().all(|p| part_type(p) == "subtask");
        let Some(agent) = self.agents.get("title").await? else {
            return Ok(None);
        };
        let (provider_id, model_id) = match &agent.model {
            Some(model) => (model.provider_id.clone(), model.model_id.clone()),
            None => (provider_id.to_owned(), model_id.to_owned()),
        };
        let context = if only_subtasks {
            TitleContext::Subtasks(subtasks.join("\n"))
        } else {
            TitleContext::History(history[..=index].to_vec())
        };
        let session_id = str_field(session, "id").unwrap_or_default().to_owned();
        let mut stream = self
            .model
            .stream(TitleRequest {
                session_id: session_id.clone(),
                agent,
                user: first.info.clone(),
                provider_id,
                model_id,
                instruction: TITLE_INSTRUCTION,
                context,
            })
            .await?;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::TextDelta { text: delta, .. }) => text.push_str(&delta),
                Ok(_) => {}
                Err(error) => return Err(LegacyError::Unknown(error.to_string())),
            }
        }
        let Some(title) = clean_title(&text) else {
            return Ok(None);
        };
        if let Err(error) = self.store.set_title(&session_id, &title).await {
            tracing::error!(%error, "failed to generate title");
        }
        Ok(Some(title))
    }
}
