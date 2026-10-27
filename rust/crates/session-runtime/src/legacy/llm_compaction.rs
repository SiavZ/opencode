//! Purpose: prepare real legacy compaction turns using the existing selection and prompt algebra.
//! Scope: persisted V1 compaction request, prior summaries, preserved recent tail and plugin context.
//! Invariants: no V2 compaction delegation; tail pointers are actual V1 message IDs.
//! Contract tests: runtime/tests/legacy_model.rs

use std::collections::HashSet;

use opencode_llm_api::Message;
use serde_json::{Value, json};

use super::compaction::{completed_compactions, preserve_recent_budget, select, serialize};
use super::llm_loop::LlmLegacyLoop;
use super::policy::Limits;
use super::types::{LegacyError, Object, WithParts, str_field};

pub(super) struct Prepared {
    pub messages: Vec<Message>,
    pub tail: Option<String>,
}

pub(super) async fn prepare(
    host: &LlmLegacyLoop,
    session: &str,
    history: &[WithParts],
    model: &Object,
) -> Result<Prepared, LegacyError> {
    let history = &history[..history.len().saturating_sub(1)];
    let prior = completed_compactions(history);
    let hidden = prior
        .iter()
        .flat_map(|item| [item.user_index, item.assistant_index])
        .collect::<HashSet<_>>();
    let messages = history
        .iter()
        .enumerate()
        .filter(|(index, _)| !hidden.contains(index))
        .map(|(_, message)| message.clone())
        .collect::<Vec<_>>();
    let budget = preserve_recent_budget(
        host.compaction
            .get("preserve_recent_tokens")
            .and_then(Value::as_f64),
        host.compaction.get("reserved").and_then(Value::as_f64),
        Limits {
            context: model["limit"]["context"].as_f64().unwrap_or_default(),
            input: model["limit"]["input"].as_f64(),
            output: model["limit"]["output"].as_f64().unwrap_or_default(),
        },
        None,
    );
    let selected = select(
        &messages,
        host.compaction
            .get("preserve_recent_turns")
            .and_then(Value::as_i64),
        budget,
        &|messages| {
            crate::compaction::estimate_tokens(
                &messages
                    .iter()
                    .map(serialize)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        },
    );
    let compacting = host
        .hooks
        .trigger(
            "experimental.session.compacting",
            json!({"sessionID":session}),
            json!({"context":[]}),
        )
        .await?;
    let transformed = host
        .hooks
        .trigger(
            "experimental.chat.messages.transform",
            json!({}),
            json!({"messages":messages[..selected.head_len]}),
        )
        .await?;
    let head = transformed
        .get("messages")
        .cloned()
        .map(serde_json::from_value::<Vec<WithParts>>)
        .transpose()
        .map_err(|error| LegacyError::Unknown(error.to_string()))?
        .unwrap_or_else(|| messages[..selected.head_len].to_vec());
    let conversation = head
        .iter()
        .map(serialize)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = match str_field(compacting.as_object().unwrap_or(&Object::new()), "prompt") {
        Some(prompt) => {
            format!("{prompt}\n\nThe following is the conversation history:\n{conversation}")
        }
        None => std::iter::once(crate::compaction::build_prompt(
            prior.last().and_then(|item| item.summary.as_deref()),
            &[conversation],
        ))
        .chain(
            compacting["context"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        )
        .collect::<Vec<_>>()
        .join("\n\n"),
    };
    Ok(Prepared {
        messages: vec![Message::user(prompt)],
        tail: selected.tail_start_id,
    })
}
