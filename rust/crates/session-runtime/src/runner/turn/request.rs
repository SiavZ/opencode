//! Purpose: llm.ts request assembly for one provider turn: projected runner history reload
//! (`SessionHistory.entriesForRunner`), session headers, `promptCacheKey`, system parts, last-step
//! MAX_STEPS handling and tool advertisement. Does not stream.
//! Scope: pure request building plus one read-only history query.
//! Uses:
//! rust/crates/
//! |-- context/src/history.rs
//! |-- llm-api/src/lib.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/message.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/max_steps.rs
//! |-- session-runtime/src/runner/to_llm_message.rs
//! |-- session-runtime/src/runner/turn.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- history::entries_for_runner
//! |-- indexmap::IndexMap
//! |-- serde_json::Map
//! |-- serde_json::Value::Object
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/runner/turn.rs
//! `-- session-runtime/src/runner/turn/attempt.rs
//! Invariants: headers keep the baseline insertion order (`x-opencode-session-id`,
//! `x-opencode-parent-session-id`?, `x-session-affinity`, `X-Session-Id`, `x-parent-session-id`?);
//! `promptCacheKey` strips `ses_` only from 64-hex Session IDs; the last step advertises no tools,
//! sets `toolChoice: none` and appends the MAX_STEPS assistant message after history.
//! Contract tests: session-runtime/tests/runner_turn.rs, session-runtime/tests/runner_turn_steps.rs

use indexmap::IndexMap;
use opencode_context::history;
use opencode_llm_api::{
    HttpOptions, LlmRequest, Model, ProviderOptions, SystemPart, ToolDefinition,
};
use opencode_session_api::message::from_row as message_from_row;
use opencode_session_api::{Message, MessageID, SessionError, SessionID, SessionInfo};
use opencode_storage_api::{DatabaseExt, Sql};
use serde_json::{Map, json};

use crate::projection::SessionEvents;
use crate::runner::max_steps::{last_step_tool_choice, max_steps_message};
use crate::runner::to_llm_message::to_llm_messages;

use super::context_error;

/// Inputs of one provider request.
pub(super) struct RequestInput<'a> {
    pub session: &'a SessionInfo,
    pub model: Model,
    pub agent_system: Option<&'a str>,
    pub baseline: &'a str,
    pub context: &'a [Message],
    pub is_last_step: bool,
    pub tools: Vec<ToolDefinition>,
}

/// `LLM.request({...})` as built by llm.ts `runTurnAttempt`.
pub(super) fn build(input: RequestInput<'_>) -> LlmRequest {
    let session = input.session;
    let id = session.id.as_str();
    let parent = session.parent_id.as_ref().map(|parent| parent.as_str());
    let mut headers = IndexMap::new();
    headers.insert("x-opencode-session-id".to_owned(), id.to_owned());
    if let Some(parent) = parent {
        headers.insert("x-opencode-parent-session-id".to_owned(), parent.to_owned());
    }
    headers.insert("x-session-affinity".to_owned(), id.to_owned());
    headers.insert("X-Session-Id".to_owned(), id.to_owned());
    if let Some(parent) = parent {
        headers.insert("x-parent-session-id".to_owned(), parent.to_owned());
    }
    let mut provider_options = ProviderOptions::new();
    let mut openai = Map::new();
    openai.insert("promptCacheKey".into(), json!(prompt_cache_key(id)));
    provider_options.insert("openai".into(), openai);

    let mut messages = to_llm_messages(input.context, &input.model);
    if input.is_last_step {
        messages.push(max_steps_message());
    }
    let mut request = LlmRequest::new(input.model);
    request.http = Some(HttpOptions {
        body: None,
        headers: Some(headers),
        query: None,
    });
    request.provider_options = Some(provider_options);
    request.system = [input.agent_system, Some(input.baseline)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .map(SystemPart::text)
        .collect();
    request.messages = messages;
    request.tools = input.tools;
    request.tool_choice = last_step_tool_choice(input.is_last_step);
    request
}

/// `/^ses_[0-9a-f]{64}$/.test(id) ? id.slice(4) : id`.
pub fn prompt_cache_key(id: &str) -> &str {
    match id.strip_prefix("ses_") {
        Some(rest)
            if rest.len() == 64
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            rest
        }
        _ => id,
    }
}

/// `SessionHistory.entriesForRunner(db, sessionID, baselineSeq)` decoded into `(seq, Message)`.
pub(super) async fn entries(
    events: &dyn SessionEvents,
    session_id: &SessionID,
    baseline_seq: i64,
) -> Result<Vec<(i64, Message)>, SessionError> {
    let id = session_id.as_str().to_owned();
    let rows = events
        .database()
        .read(move |sql: &dyn Sql| history::entries_for_runner(sql, &id, baseline_seq))
        .await
        .map_err(|error| context_error(session_id, error))?;
    rows.into_iter()
        .map(|row| {
            let data = match row.data {
                serde_json::Value::Object(map) => map,
                _ => Map::new(),
            };
            let message =
                message_from_row(&row.id, &row.message_type, data).map_err(
                    |_| match MessageID::new(row.id.as_str()) {
                        Ok(message_id) => SessionError::MessageDecode {
                            session_id: session_id.clone(),
                            message_id,
                        },
                        Err(_) => SessionError::Storage {
                            message: format!("invalid message id {}", row.id),
                            retryable: false,
                        },
                    },
                )?;
            Ok((row.seq, message))
        })
        .collect()
}
