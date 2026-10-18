//! Purpose: fixture tests for internal V2 compaction (packages/core/src/session/compaction.ts):
//! serialization, tail selection, prompt building, compactAfterOverflow event publishing with a
//! deterministic fake provider stream, provider-error and empty-summary no-ops, and compactIfNeeded.
//! Scope: test-only; real SQLite + event store + Session projectors, fixture LLM client and route.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/compaction.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- futures::stream
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: the provider stream is a labeled fixture (`ScriptedLlm`); no network.
//! Contract tests: session-runtime/tests/compaction.rs

#[path = "support/sql.rs"]
mod sql;

use std::sync::{Arc, Mutex};

use opencode_llm_api::{
    FinishReason, LlmError, LlmEvent, LlmEventStream, LlmRequest, Model, ModelLimits,
    PreparedRequest, ProviderId, Route, RouteDefaults,
};
use opencode_schema::session_message::Message;
use opencode_session_api::SessionID;
use opencode_session_runtime::compaction::{
    Compaction, Configured, Settings, build_prompt, estimate_tokens, select, serialize, settings,
    truncate,
};
use serde_json::json;

const SESSION: &str = "ses_compaction";

/// Fixture route carrying only limits.
#[derive(Debug)]
struct LimitRoute(RouteDefaults);

impl Route for LimitRoute {
    fn id(&self) -> &str {
        "fixture"
    }
    fn protocol(&self) -> &str {
        "fixture"
    }
    fn transport(&self) -> &str {
        "fixture"
    }
    fn provider(&self) -> Option<&ProviderId> {
        None
    }
    fn defaults(&self) -> &RouteDefaults {
        &self.0
    }
    fn prepare(&self, _: &LlmRequest) -> Result<PreparedRequest, LlmError> {
        Err(LlmError::invalid_request("fixture"))
    }
    fn stream(&self, _: LlmRequest) -> LlmEventStream {
        Box::pin(futures::stream::empty())
    }
}

/// Fixture LLM client: records requests and replays a scripted event list.
struct ScriptedLlm {
    events: Vec<LlmEvent>,
    requests: Mutex<Vec<LlmRequest>>,
}

impl opencode_llm_api::LlmClient for ScriptedLlm {
    fn prepare(&self, _: LlmRequest) -> Result<PreparedRequest, LlmError> {
        Err(LlmError::invalid_request("fixture"))
    }
    fn stream(&self, request: LlmRequest) -> LlmEventStream {
        self.requests.lock().unwrap().push(request);
        Box::pin(futures::stream::iter(
            self.events.clone().into_iter().map(Ok),
        ))
    }
}

fn request(context: u64, output: u64) -> LlmRequest {
    let defaults = RouteDefaults {
        limits: Some(ModelLimits {
            context: Some(context),
            output: Some(output),
        }),
        ..RouteDefaults::default()
    };
    LlmRequest::new(Model::new("m", "p", Arc::new(LimitRoute(defaults))))
}

fn message(value: serde_json::Value) -> Message {
    serde_json::from_value(value).unwrap()
}

fn user(id: &str, text: &str) -> Message {
    message(json!({ "id": id, "type": "user", "text": text, "time": { "created": 1 } }))
}

fn delta(text: &str) -> LlmEvent {
    LlmEvent::TextDelta {
        id: "t".into(),
        text: text.into(),
        provider_metadata: None,
    }
}

fn finish() -> LlmEvent {
    LlmEvent::Finish {
        reason: FinishReason::Stop,
        usage: None,
        provider_metadata: None,
    }
}

async fn event_types(h: &sql::Harness) -> Vec<String> {
    h.query("SELECT type FROM event ORDER BY seq", vec![])
        .await
        .iter()
        .map(|row| row.get::<String>("type").unwrap())
        .collect()
}

#[test]
fn serialize_matches_baseline_shapes() {
    let assistant = message(json!({
        "id": "msg_a", "type": "assistant", "agent": "build", "time": { "created": 1 },
        "model": { "id": "m", "providerID": "p" },
        "content": [
            { "type": "text", "id": "t", "text": "hi" },
            { "type": "reasoning", "id": "r", "text": "" },
            { "type": "tool", "id": "c", "name": "read", "time": { "created": 1 },
              "state": { "status": "completed", "input": { "path": "a" }, "structured": {},
                         "content": [{ "type": "text", "text": "out" }, { "type": "file", "uri": "u", "mime": "image/png" }] } },
        ],
    }));
    assert_eq!(
        serialize(&assistant),
        "[Assistant]: hi\n[Assistant tool call]: read({\"path\":\"a\"})\n[Tool result]: out\n[Attached image/png]"
    );
    let shell = message(
        json!({ "id": "msg_s", "type": "shell", "callID": "c", "command": "ls", "output": "x", "time": { "created": 1 } }),
    );
    assert_eq!(serialize(&shell), "[Shell]: ls\nx");
    assert_eq!(
        truncate(&"a".repeat(2001)),
        format!("{}\n[truncated]", "a".repeat(2000))
    );
    assert_eq!(estimate_tokens("abcdef"), 2.0);
}

#[test]
fn select_keeps_recent_tail_within_budget() {
    let entries = vec![user("msg_1", &"a".repeat(400)), user("msg_2", "short")];
    let (head, recent) = select(&entries, 10.0).unwrap();
    assert_eq!(head, format!("[User]: {}", "a".repeat(400)));
    assert_eq!(recent, "[User]: short");
    assert!(select(&[], 10.0).is_none());
    assert_eq!(
        settings(&[
            Configured {
                keep_tokens: Some(5.0),
                ..Default::default()
            },
            Configured {
                auto: Some(false),
                ..Default::default()
            }
        ]),
        Settings {
            auto: false,
            buffer: 20_000.0,
            tokens: 5.0
        }
    );
}

#[test]
fn build_prompt_includes_prior_summary_when_present() {
    let fresh = build_prompt(None, &["ctx".into()]);
    assert!(fresh.starts_with("Here is the conversation so far:\n\n<conversation>\nctx\n</conversation>\n\nCreate a new anchored summary"));
    let update = build_prompt(Some("old"), &["ctx".into()]);
    assert!(
        update.contains("<prior-summary>\nold\n</prior-summary>\n\nThe <prior-summary> summarizes")
    );
}

#[tokio::test]
async fn compact_after_overflow_publishes_started_and_ended() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let llm = ScriptedLlm {
        events: vec![delta("## Objective"), delta("\n- x"), finish()],
        requests: Mutex::default(),
    };
    let compaction = Compaction {
        events: h.events.as_ref(),
        llm: &llm,
        settings: Settings {
            tokens: 3.0,
            ..Settings::default()
        },
    };
    let entries = vec![user("msg_1", &"a".repeat(100)), user("msg_2", "hi")];
    let ok = compaction
        .compact_after_overflow(
            &SessionID::new(SESSION).unwrap(),
            &entries,
            &request(100_000, 2000),
        )
        .await
        .unwrap();
    assert!(ok);
    assert_eq!(
        event_types(&h).await,
        vec![
            "session.next.compaction.started.1",
            "session.next.compaction.ended.1"
        ]
    );
    let ended = h
        .query(
            "SELECT data FROM event WHERE type = 'session.next.compaction.ended.1'",
            vec![],
        )
        .await;
    let data: serde_json::Value = ended[0].get_json("data").unwrap();
    assert_eq!(data["text"], "## Objective\n- x");
    assert_eq!(data["recent"], "[User]: hi");
    assert_eq!(data["reason"], "auto");
    let sent = &llm.requests.lock().unwrap()[0];
    assert_eq!(sent.generation.as_ref().unwrap().max_tokens, Some(2000.0));
    assert!(sent.tools.is_empty());
}

#[tokio::test]
async fn provider_error_or_blank_summary_publishes_only_started() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let error: LlmEvent =
        serde_json::from_value(json!({ "type": "provider-error", "message": "boom" })).unwrap();
    let llm = ScriptedLlm {
        events: vec![delta("x"), error],
        requests: Mutex::default(),
    };
    let compaction = Compaction {
        events: h.events.as_ref(),
        llm: &llm,
        settings: Settings {
            tokens: 3.0,
            ..Settings::default()
        },
    };
    let entries = vec![user("msg_1", &"a".repeat(100)), user("msg_2", "hi")];
    let session = SessionID::new(SESSION).unwrap();
    assert!(
        !compaction
            .compact_after_overflow(&session, &entries, &request(100_000, 0))
            .await
            .unwrap()
    );
    assert_eq!(
        event_types(&h).await,
        vec!["session.next.compaction.started.1"]
    );
    let blank = ScriptedLlm {
        events: vec![delta("  ")],
        requests: Mutex::default(),
    };
    let compaction = Compaction {
        llm: &blank,
        ..compaction
    };
    assert!(
        !compaction
            .compact_after_overflow(&session, &entries, &request(100_000, 0))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn nothing_to_summarize_or_no_context_is_a_noop() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let llm = ScriptedLlm {
        events: vec![delta("s")],
        requests: Mutex::default(),
    };
    let compaction = Compaction {
        events: h.events.as_ref(),
        llm: &llm,
        settings: Settings::default(),
    };
    let session = SessionID::new(SESSION).unwrap();
    // Everything fits in the kept tail and there is no prior summary: nothing to compact.
    assert!(
        !compaction
            .compact_after_overflow(&session, &[user("msg_1", "hi")], &request(100_000, 0))
            .await
            .unwrap()
    );
    // Unknown context window.
    assert!(
        !compaction
            .compact_after_overflow(&session, &[user("msg_1", "hi")], &request(0, 0))
            .await
            .unwrap()
    );
    // Prompt larger than the window.
    let big = Compaction {
        settings: Settings {
            tokens: 0.0,
            ..Settings::default()
        },
        ..compaction
    };
    assert!(
        !big.compact_after_overflow(
            &session,
            &[user("msg_1", &"a".repeat(10_000))],
            &request(1_000, 0)
        )
        .await
        .unwrap()
    );
    assert!(event_types(&h).await.is_empty());
    assert!(llm.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compact_if_needed_respects_auto_and_buffer() {
    let h = sql::open().await;
    h.seed_session(SESSION).await;
    let llm = ScriptedLlm {
        events: vec![delta("s")],
        requests: Mutex::default(),
    };
    let session = SessionID::new(SESSION).unwrap();
    let entries = vec![user("msg_1", &"a".repeat(100)), user("msg_2", "hi")];
    let mut small = request(30_000, 0);
    small.messages = vec![opencode_llm_api::Message::user("hello")];
    let compaction = Compaction {
        events: h.events.as_ref(),
        llm: &llm,
        settings: Settings {
            tokens: 3.0,
            ..Settings::default()
        },
    };
    assert!(
        !compaction
            .compact_if_needed(&session, &entries, &small)
            .await
            .unwrap(),
        "fits under context - buffer"
    );
    let mut big = request(20_100, 0);
    big.messages = vec![opencode_llm_api::Message::user("a".repeat(1000))];
    assert!(
        compaction
            .compact_if_needed(&session, &entries, &big)
            .await
            .unwrap()
    );
    let off = Compaction {
        settings: Settings {
            auto: false,
            ..compaction.settings
        },
        ..compaction
    };
    assert!(
        !off.compact_if_needed(&session, &entries, &big)
            .await
            .unwrap()
    );
}
