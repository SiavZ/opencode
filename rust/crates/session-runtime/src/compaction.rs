//! Purpose: internal automatic compaction of the V2 Session runner (packages/core/src/session/
//! compaction.ts): settings fold, transcript serialization, recent-tail selection by token budget,
//! anchored summary prompt (new and update), `compactIfNeeded` pre-turn check and
//! `compactAfterOverflow` summary turn publishing `session.next.compaction.started/ended`.
//! The public V2 `compact` operation stays `OperationUnavailable` (session-api).
//! Scope: one summary turn through `opencode_llm_api::LlmClient::stream`; events through
//! `projection::publish_session_event`.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! `-- session-runtime/src/projection.rs
//! External:
//! |-- futures::StreamExt
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location.rs
//! |-- session-runtime/src/legacy/compaction.rs
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/tests/compaction.rs
//! Invariants: no events are published unless a non-empty summary prompt fits the context window;
//! `compaction.ended` is published only after a stream that produced non-blank text and no
//! provider-error event. Token estimates use `round(chars / 4)` (`Token.estimate`).
//! Contract tests: session-runtime/tests/compaction.rs

use futures::StreamExt;
use opencode_llm_api::{GenerationOptions, LlmClient, LlmEvent, LlmRequest, Message as LlmMessage};
use opencode_schema::prompt::ToolContent;
use opencode_schema::session_event::{CompactionEnded, CompactionStarted};
use opencode_schema::session_message::{AssistantContent, CompactionReason, Message, ToolState};
use opencode_schema::values::DateTimeUtc;
use opencode_session_api::{MessageID, SessionError, SessionID};
use serde_json::Value;

use crate::projection::{SessionEvents, publish_session_event};

pub const DEFAULT_BUFFER: f64 = 20_000.0;
pub const DEFAULT_KEEP_TOKENS: f64 = 8_000.0;
const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
const SUMMARY_OUTPUT_TOKENS: f64 = 4_096.0;

pub const SUMMARY_TEMPLATE: &str = "Output exactly the Markdown structure shown inside <template> and keep the section order unchanged. Do not include the <template> tags in your response.
<template>
## Objective
- [one or two brief sentences describing what the user is trying to accomplish]

## Important Details
- [constraints/preferences, decisions and why, important facts/assumptions, exact context needed to continue, or \"(none)\"]

## Work State
### Completed
- [finished work, verified facts, or changes made; otherwise \"(none)\"]

### Active
- [current work, partial changes, or investigation state; otherwise \"(none)\"]

### Blocked
- [blockers, failing commands, or unknowns; otherwise \"(none)\"]

## Next Move
1. [immediate concrete action, or \"(none)\"]
2. [next action if known, or \"(none)\"]

## Relevant Files
- [file or directory path: why it matters, or \"(none)\"]
</template>

Rules:
- Keep every section, even when empty.
- Use terse bullets, not prose paragraphs.
- Preserve exact file paths, symbols, commands, error strings, URLs, and identifiers when known.
- Do not mention the summary process or that context was compacted.";

pub const SUMMARY_UPDATE_INSTRUCTIONS: &str = "The <prior-summary> summarizes everything that happened before the <conversation>. Construct a new summary that combines both. The <prior-summary> is discarded after this: anything you do not carry into the new summary is lost.

When combining:
- Carry forward objectives, constraints, user directives, decisions, and parallel workstreams from the <prior-summary> even when the <conversation> does not mention them. Drop only what is finished and no longer needed.
- The <conversation> is more recent than the <prior-summary>. Where they conflict, the conversation wins: state the corrected fact and drop the old claim.
- Add new progress, decisions, constraints, and context from the conversation.
- Move completed work from \"Active\" to \"Completed\".
- If a blocker has been resolved, update the summary to reflect that while keeping any details still needed to continue the work.
- Update \"Objective\" and \"Next Move\" to reflect the current work state.";

/// `Token.estimate`: `max(0, round(len / 4))` over UTF-16 length.
pub fn estimate_tokens(text: &str) -> f64 {
    (text.encode_utf16().count() as f64 / 4.0).round().max(0.0)
}

/// `truncate`: cut to 2000 UTF-16 units with a `[truncated]` marker.
pub fn truncate(value: &str) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    if units.len() <= TOOL_OUTPUT_MAX_CHARS {
        return value.to_owned();
    }
    format!(
        "{}\n[truncated]",
        String::from_utf16_lossy(&units[..TOOL_OUTPUT_MAX_CHARS])
    )
}

/// `serializeToolContent`.
pub fn serialize_tool_content(content: &[ToolContent]) -> String {
    content
        .iter()
        .map(|item| match item {
            ToolContent::Text { text } => text.clone(),
            ToolContent::File { mime, name, .. } => match name {
                Some(name) => format!("[Attached {mime}: {name}]"),
                None => format!("[Attached {mime}]"),
            },
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn input_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `serialize(message)`.
pub fn serialize(message: &Message) -> String {
    match message {
        Message::User(user) => {
            let mut lines = vec![format!("[User]: {}", user.text)];
            lines.extend(user.files.iter().flatten().map(|file| {
                format!(
                    "[Attached {}: {}]",
                    file.mime,
                    file.name.as_deref().unwrap_or(&file.uri)
                )
            }));
            lines.join("\n")
        }
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .flat_map(|part| match part {
                AssistantContent::Text(text) => vec![format!("[Assistant]: {}", text.text)],
                AssistantContent::Reasoning(reasoning) if reasoning.text.is_empty() => vec![],
                AssistantContent::Reasoning(reasoning) => {
                    vec![format!("[Assistant reasoning]: {}", reasoning.text)]
                }
                AssistantContent::Tool(tool) => {
                    let input = match &tool.state {
                        ToolState::Pending(state) => state.input.clone(),
                        ToolState::Running(state) => {
                            input_text(&Value::Object(state.input.clone().into_iter().collect()))
                        }
                        ToolState::Completed(state) => {
                            input_text(&Value::Object(state.input.clone().into_iter().collect()))
                        }
                        ToolState::Error(state) => {
                            input_text(&Value::Object(state.input.clone().into_iter().collect()))
                        }
                    };
                    let call = format!("[Assistant tool call]: {}({input})", tool.name);
                    match &tool.state {
                        ToolState::Completed(state) => vec![
                            call,
                            format!(
                                "[Tool result]: {}",
                                truncate(&serialize_tool_content(&state.content))
                            ),
                        ],
                        ToolState::Error(state) => {
                            vec![call, format!("[Tool error]: {}", state.error.message)]
                        }
                        _ => vec![call],
                    }
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Message::System(system) => format!("[System update]: {}", system.text),
        Message::Synthetic(synthetic) => format!("[Synthetic context]: {}", synthetic.text),
        Message::Shell(shell) => format!("[Shell]: {}\n{}", shell.command, truncate(&shell.output)),
        _ => String::new(),
    }
}

/// Folded `compaction` config (`settings`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub auto: bool,
    pub buffer: f64,
    pub tokens: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto: true,
            buffer: DEFAULT_BUFFER,
            tokens: DEFAULT_KEEP_TOKENS,
        }
    }
}

/// One config document's `compaction` block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Configured {
    pub auto: Option<bool>,
    pub buffer: Option<f64>,
    pub keep_tokens: Option<f64>,
}

/// `settings(documents)`: later documents override earlier ones field by field.
pub fn settings(documents: &[Configured]) -> Settings {
    documents
        .iter()
        .fold(Settings::default(), |result, current| Settings {
            auto: current.auto.unwrap_or(result.auto),
            buffer: current.buffer.unwrap_or(result.buffer),
            tokens: current.keep_tokens.unwrap_or(result.tokens),
        })
}

/// `select(entries, tokens)`: serialized head and the most recent tail within `tokens`.
pub fn select(entries: &[Message], tokens: f64) -> Option<(String, String)> {
    let conversation: Vec<String> = entries
        .iter()
        .filter(|message| !matches!(message, Message::Compaction(_)))
        .map(serialize)
        .filter(|text| !text.is_empty())
        .collect();
    if conversation.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut split = conversation.len();
    for index in (0..conversation.len()).rev() {
        let next = total + estimate_tokens(&conversation[index]);
        if next > tokens {
            break;
        }
        total = next;
        split = index;
    }
    Some((
        conversation[..split].join("\n\n"),
        conversation[split..].join("\n\n"),
    ))
}

/// `buildPrompt`.
pub fn build_prompt(previous_summary: Option<&str>, context: &[String]) -> String {
    let conversation = format!(
        "Here is the conversation so far:\n\n<conversation>\n{}\n</conversation>",
        context.join("\n\n")
    );
    match previous_summary.filter(|summary| !summary.is_empty()) {
        None => [
            conversation.as_str(),
            "Create a new anchored summary from the conversation history in the <conversation> tags above so another coding agent can continue the work.",
            SUMMARY_TEMPLATE,
        ]
        .join("\n\n"),
        Some(previous) => [
            conversation,
            format!(
                "Here is the summary of the conversation before the <conversation> above:\n\n<prior-summary>\n{previous}\n</prior-summary>"
            ),
            SUMMARY_UPDATE_INSTRUCTIONS.to_owned(),
            SUMMARY_TEMPLATE.to_owned(),
        ]
        .join("\n\n"),
    }
}

fn context_limit(request: &LlmRequest) -> Option<f64> {
    route_limits(request)
        .and_then(|limits| limits.context)
        .map(|context| context as f64)
        .filter(|context| *context > 0.0)
}

fn route_limits(request: &LlmRequest) -> Option<&opencode_llm_api::ModelLimits> {
    request.model.route.defaults().limits.as_ref()
}

fn output_limit(request: &LlmRequest) -> f64 {
    request
        .generation
        .as_ref()
        .and_then(|generation| generation.max_tokens)
        .or_else(|| {
            route_limits(request)
                .and_then(|limits| limits.output)
                .map(|output| output as f64)
        })
        .unwrap_or(0.0)
}

/// `SessionCompaction.make(...)`: the runner's compaction service.
pub struct Compaction<'a> {
    pub events: &'a dyn SessionEvents,
    pub llm: &'a dyn LlmClient,
    pub settings: Settings,
}

impl Compaction<'_> {
    /// `compactAfterOverflow`: run one summary turn. Returns whether compaction ended.
    pub async fn compact_after_overflow(
        &self,
        session: &SessionID,
        entries: &[Message],
        request: &LlmRequest,
    ) -> Result<bool, SessionError> {
        let Some(context) = context_limit(request) else {
            return Ok(false);
        };
        let output = output_limit(request);
        let Some((head, recent)) = select(entries, self.settings.tokens) else {
            return Ok(false);
        };
        let previous = entries.iter().find_map(|message| match message {
            Message::Compaction(compaction) => Some(compaction),
            _ => None,
        });
        if head.is_empty() && previous.is_none() {
            return Ok(false);
        }
        let context_parts: Vec<String> =
            [previous.map(|p| p.recent.clone()).unwrap_or_default(), head]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
        let prompt = build_prompt(previous.map(|p| p.summary.as_str()), &context_parts);
        let summary_output = if output == 0.0 {
            SUMMARY_OUTPUT_TOKENS
        } else {
            output
        }
        .min(SUMMARY_OUTPUT_TOKENS);
        if estimate_tokens(&prompt) > context - summary_output {
            return Ok(false);
        }
        let message_id = MessageID::create();
        publish_session_event(
            self.events,
            &CompactionStarted {
                timestamp: DateTimeUtc::now(),
                session_id: session.clone(),
                message_id: message_id.clone(),
                reason: CompactionReason::Auto,
            },
        )
        .await?;
        let mut summary_request = LlmRequest::new(request.model.clone());
        summary_request.http = request.http.clone();
        summary_request.messages = vec![LlmMessage::user(prompt)];
        summary_request.generation = Some(GenerationOptions {
            max_tokens: Some(summary_output),
            ..GenerationOptions::default()
        });
        let mut stream = self.llm.stream(summary_request);
        let mut chunks = String::new();
        let mut failed = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::ProviderError { .. }) => failed = true,
                Ok(LlmEvent::TextDelta { text, .. }) => chunks.push_str(&text),
                Ok(_) => {}
                Err(_) => return Ok(false),
            }
        }
        if failed || chunks.trim().is_empty() {
            return Ok(false);
        }
        publish_session_event(
            self.events,
            &CompactionEnded {
                timestamp: DateTimeUtc::now(),
                session_id: session.clone(),
                message_id,
                reason: CompactionReason::Auto,
                text: chunks,
                recent,
            },
        )
        .await?;
        Ok(true)
    }

    /// `compactIfNeeded`: compacts when the estimated request exceeds `context - max(output, buffer)`.
    pub async fn compact_if_needed(
        &self,
        session: &SessionID,
        entries: &[Message],
        request: &LlmRequest,
    ) -> Result<bool, SessionError> {
        if !self.settings.auto {
            return Ok(false);
        }
        let Some(context) = context_limit(request) else {
            return Ok(false);
        };
        let output = output_limit(request);
        let size = serde_json::json!({
            "system": request.system,
            "messages": request.messages,
            "tools": request.tools,
        });
        if estimate_tokens(&size.to_string()) <= context - output.max(self.settings.buffer) {
            return Ok(false);
        }
        self.compact_after_overflow(session, entries, request).await
    }
}
