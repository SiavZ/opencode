//! Purpose: legacy `SessionCompaction` (packages/opencode/src/session/compaction.ts): transcript
//! serialization, completed-compaction detection, turn split and recent-tail selection with the
//! preserve-recent budget, tool-output pruning, overflow replay selection, compaction request
//! creation (`create`) and the auto-continue message. The summary turn itself (`process` through
//! `SessionProcessor`) needs the legacy processor/LLM loop and fails typed `NotYetPorted`.
//! Scope: pure selection functions plus `LegacyStore` writes for prune/create/continue.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/legacy/policy.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- str::trim
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/compaction_legacy.rs
//! Invariants: prune never touches the last two user turns, stops at a summary assistant or an
//! already-compacted tool part, skips protected tools (`skill`), and only writes when more than
//! 20k tokens beyond the 40k protected window would be freed. Tail selection never returns a tail
//! starting at index 0. Replay picks the last real user message before the compaction parent and
//! falls back when no earlier real user message would remain.
//! Contract tests: session-runtime/tests/compaction_legacy.rs

use serde_json::{Value, json};

use super::policy::{Limits, usable};
use super::store::{LegacyStore, object};
use super::types::{
    LegacyError, Object, WithParts, message_id, now_millis, part_id, part_type, str_field,
};
use crate::compaction::{estimate_tokens, truncate};

pub const PRUNE_MINIMUM: f64 = 20_000.0;
pub const PRUNE_PROTECT: f64 = 40_000.0;
pub const PRUNE_PROTECTED_TOOLS: [&str; 1] = ["skill"];
const MIN_PRESERVE_RECENT_TOKENS: f64 = 2_000.0;
const MAX_PRESERVE_RECENT_TOKENS: f64 = 15_000.0;

/// Legacy `serialize(message)`.
pub fn serialize(message: &WithParts) -> String {
    if message.is_user() {
        let text = message
            .parts
            .iter()
            .filter(|part| {
                part_type(part) == "text" && part.get("ignored") != Some(&Value::Bool(true))
            })
            .filter_map(|part| str_field(part, "text"))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let files = message
            .parts
            .iter()
            .filter(|part| part_type(part) == "file")
            .map(|part| {
                format!(
                    "[Attached {}: {}]",
                    str_field(part, "mime").unwrap_or_default(),
                    str_field(part, "filename").unwrap_or("file")
                )
            });
        return (!text.is_empty())
            .then(|| format!("[User]: {text}"))
            .into_iter()
            .chain(files)
            .collect::<Vec<_>>()
            .join("\n");
    }
    message
        .parts
        .iter()
        .flat_map(|part| match part_type(part) {
            "text" => str_field(part, "text")
                .filter(|text| !text.is_empty())
                .map(|text| vec![format!("[Assistant]: {text}")])
                .unwrap_or_default(),
            "reasoning" => str_field(part, "text")
                .filter(|text| !text.is_empty())
                .map(|text| vec![format!("[Assistant reasoning]: {text}")])
                .unwrap_or_default(),
            "tool" => {
                let state = &part["state"];
                let call = format!(
                    "[Assistant tool call]: {}({})",
                    str_field(part, "tool").unwrap_or_default(),
                    state.get("input").unwrap_or(&Value::Null)
                );
                match state["status"].as_str() {
                    Some("completed") => {
                        let output =
                            if state["time"].get("compacted").is_some_and(|v| !v.is_null()) {
                                "[Old tool result content cleared]".to_owned()
                            } else {
                                let attachments =
                                    state["attachments"].as_array().into_iter().flatten().map(
                                        |item| {
                                            format!(
                                                "[Attached {}: {}]",
                                                item["mime"].as_str().unwrap_or_default(),
                                                item["filename"].as_str().unwrap_or("file")
                                            )
                                        },
                                    );
                                truncate(
                                    &std::iter::once(
                                        state["output"].as_str().unwrap_or_default().to_owned(),
                                    )
                                    .chain(attachments)
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                )
                            };
                        vec![call, format!("[Tool result]: {output}")]
                    }
                    Some("error") => vec![
                        call,
                        format!(
                            "[Tool error]: {}",
                            state["error"].as_str().unwrap_or_default()
                        ),
                    ],
                    _ => vec![call],
                }
            }
            _ => vec![],
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `summaryText`: trimmed text parts joined by blank lines, `None` when empty.
pub fn summary_text(message: &WithParts) -> Option<String> {
    let text = message
        .parts
        .iter()
        .filter(|part| part_type(part) == "text")
        .filter_map(|part| str_field(part, "text"))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// One finished compaction pair (`CompletedCompaction`).
#[derive(Clone, Debug, PartialEq)]
pub struct CompletedCompaction {
    pub user_index: usize,
    pub assistant_index: usize,
    pub summary: Option<String>,
}

/// `completedCompactions`.
pub fn completed_compactions(messages: &[WithParts]) -> Vec<CompletedCompaction> {
    let users: std::collections::HashMap<&str, usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_user() && m.has_part("compaction"))
        .map(|(index, m)| (m.id(), index))
        .collect();
    messages
        .iter()
        .enumerate()
        .filter_map(|(assistant_index, m)| {
            if !m.is_assistant() {
                return None;
            }
            let truthy = |key: &str| {
                m.info
                    .get(key)
                    .is_some_and(|v| !v.is_null() && v != &Value::Bool(false) && v != "")
            };
            if !truthy("summary") || !truthy("finish") || truthy("error") {
                return None;
            }
            let user_index = *users.get(str_field(&m.info, "parentID").unwrap_or_default())?;
            Some(CompletedCompaction {
                user_index,
                assistant_index,
                summary: summary_text(m),
            })
        })
        .collect()
}

/// `turns`: `[start, end)` of each non-compaction user turn.
pub fn turns(messages: &[WithParts]) -> Vec<(usize, usize, String)> {
    let starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_user() && !m.has_part("compaction"))
        .map(|(index, _)| index)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, start)| {
            let end = starts.get(i + 1).copied().unwrap_or(messages.len());
            (*start, end, messages[*start].id().to_owned())
        })
        .collect()
}

/// `preserveRecentBudget`.
pub fn preserve_recent_budget(
    configured: Option<f64>,
    reserved: Option<f64>,
    limits: Limits,
    output_max: Option<f64>,
) -> f64 {
    configured.unwrap_or_else(|| {
        MAX_PRESERVE_RECENT_TOKENS.min(
            MIN_PRESERVE_RECENT_TOKENS.max((usable(reserved, limits, output_max) * 0.25).floor()),
        )
    })
}

/// `select` result: messages to summarize and the first retained message.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub head_len: usize,
    pub tail_start_id: Option<String>,
}

/// `select({ messages, cfg, model })`. `estimate` is `Token.estimate(JSON.stringify(toModelMessages(slice)))`.
pub fn select(
    messages: &[WithParts],
    tail_turns: Option<i64>,
    budget: f64,
    estimate: &dyn Fn(&[WithParts]) -> f64,
) -> Selection {
    let all = Selection {
        head_len: messages.len(),
        tail_start_id: None,
    };
    if tail_turns.is_some_and(|limit| limit <= 0) {
        return all;
    }
    let turns = turns(messages);
    if turns.is_empty() {
        return all;
    }
    let recent = match tail_turns {
        Some(limit) => &turns[turns.len().saturating_sub(limit as usize)..],
        None => &turns[..],
    };
    let mut total = 0.0;
    let mut keep: Option<(usize, String)> = None;
    for (start, end, id) in recent.iter().rev() {
        let size = estimate(&messages[*start..*end]);
        if total + size <= budget {
            total += size;
            keep = Some((*start, id.clone()));
            continue;
        }
        let remaining = budget - total;
        if remaining > 0.0
            && end - start > 1
            && let Some(split) =
                (start + 1..*end).find(|split| estimate(&messages[*split..*end]) <= remaining)
        {
            keep = Some((split, messages[split].id().to_owned()));
        }
        break;
    }
    match keep {
        Some((start, id)) if start != 0 => Selection {
            head_len: start,
            tail_start_id: Some(id),
        },
        _ => all,
    }
}

/// Prune candidate selection: IDs (message, part) of completed tool parts to mark compacted.
pub fn prune_candidates(messages: &[WithParts]) -> (Vec<(usize, usize)>, f64, f64) {
    let mut total = 0.0;
    let mut pruned = 0.0;
    let mut found = Vec::new();
    let mut user_turns = 0;
    'outer: for (mi, message) in messages.iter().enumerate().rev() {
        if message.is_user() {
            user_turns += 1;
        }
        if user_turns < 2 {
            continue;
        }
        if message.is_assistant()
            && message
                .info
                .get("summary")
                .is_some_and(|v| v == &Value::Bool(true))
        {
            break;
        }
        for (pi, part) in message.parts.iter().enumerate().rev() {
            if part_type(part) != "tool" || part["state"]["status"] != "completed" {
                continue;
            }
            if PRUNE_PROTECTED_TOOLS.contains(&str_field(part, "tool").unwrap_or_default()) {
                continue;
            }
            if part["state"]["time"]
                .get("compacted")
                .is_some_and(|v| !v.is_null() && v != 0)
            {
                break 'outer;
            }
            let estimate = estimate_tokens(part["state"]["output"].as_str().unwrap_or_default());
            total += estimate;
            if total <= PRUNE_PROTECT {
                continue;
            }
            pruned += estimate;
            found.push((mi, pi));
        }
    }
    (found, pruned, total)
}

/// Overflow replay: index of the user message to replay; history is `messages[..index]`.
pub fn replay_split(messages: &[WithParts], parent_id: &str) -> Option<usize> {
    let index = messages.iter().position(|m| m.id() == parent_id)?;
    let replay = (0..index)
        .rev()
        .find(|i| messages[*i].is_user() && !messages[*i].has_part("compaction"))?;
    let has_content = messages[..replay]
        .iter()
        .any(|m| m.is_user() && !m.has_part("compaction"));
    has_content.then_some(replay)
}

/// Auto-continue text after a successful automatic compaction without replay.
pub fn continue_text(overflow: bool) -> String {
    let prefix = if overflow {
        "The previous request exceeded the provider's size limit due to large media attachments. The conversation was compacted and media files were removed from context. If the user was asking about attached images or files, explain that the attachments were too large to process and suggest they try again with smaller or fewer files.\n\n"
    } else {
        ""
    };
    format!(
        "{prefix}Continue if you have next steps, or stop and ask for clarification if you are unsure how to proceed."
    )
}

/// `SessionCompaction` writes that do not need the processor.
pub struct LegacyCompaction<'a> {
    pub store: &'a LegacyStore,
}

impl LegacyCompaction<'_> {
    /// `prune({ sessionID })` when `cfg.compaction.prune` is enabled.
    pub async fn prune(&self, session: &str, enabled: bool) -> Result<usize, LegacyError> {
        if !enabled {
            return Ok(0);
        }
        let messages = match self.store.messages(session).await {
            Ok(messages) => messages,
            Err(LegacyError::NotFound(_)) => return Ok(0),
            Err(error) => return Err(error),
        };
        let (found, pruned, _) = prune_candidates(&messages);
        if pruned <= PRUNE_MINIMUM {
            return Ok(0);
        }
        for (mi, pi) in &found {
            let mut part = messages[*mi].parts[*pi].clone();
            part["state"]["time"]["compacted"] = json!(now_millis());
            self.store.update_part(&part).await?;
        }
        Ok(found.len())
    }

    /// `create({ sessionID, agent, model, auto, overflow? })`: compaction request user message.
    pub async fn create(
        &self,
        session: &str,
        agent: &str,
        model: Value,
        auto: bool,
        overflow: Option<bool>,
    ) -> Result<Object, LegacyError> {
        let message = object(json!({
            "id": message_id(),
            "role": "user",
            "model": model,
            "sessionID": session,
            "agent": agent,
            "time": { "created": now_millis() },
        }));
        self.store.update_message(&message).await?;
        let mut part = object(json!({
            "id": part_id(),
            "messageID": message["id"],
            "sessionID": session,
            "type": "compaction",
            "auto": auto,
            "overflow": overflow,
        }));
        part.retain(|_, value| !value.is_null());
        self.store.update_part(&part).await?;
        Ok(message)
    }

    /// Auto-continue user message (`compaction_continue` marker).
    pub async fn continue_after(
        &self,
        session: &str,
        user: &Object,
        overflow: bool,
    ) -> Result<(), LegacyError> {
        let message = object(json!({
            "id": message_id(),
            "role": "user",
            "sessionID": session,
            "time": { "created": now_millis() },
            "agent": user.get("agent"),
            "model": user.get("model"),
        }));
        self.store.update_message(&message).await?;
        let now = now_millis();
        self.store
            .update_part(&object(json!({
                "id": part_id(),
                "messageID": message["id"],
                "sessionID": session,
                "type": "text",
                "metadata": { "compaction_continue": true },
                "synthetic": true,
                "text": continue_text(overflow),
                "time": { "start": now, "end": now },
            })))
            .await
    }

    /// `process(...)`: the summary turn runs through the legacy processor, not ported yet.
    pub async fn process(&self) -> Result<&'static str, LegacyError> {
        Err(LegacyError::NotYetPorted(
            "SessionCompaction.process (legacy SessionProcessor summary turn)",
        ))
    }
}
