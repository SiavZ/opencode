//! Purpose: fixture tests for legacy title generation (`ensureTitle`): eligibility, subtask-only
//! context, `<think>` stripping, truncation and the persisted title, using a deterministic fake
//! provider stream.
//! Scope: test-only; real SQLite + legacy projectors, fixture agents and title model.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/title.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::stream::iter
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: `ScriptedTitle` is a labeled fixture stream; no network.
//! Contract tests: session-runtime/tests/title.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_llm_api::{LlmEvent, LlmEventStream};
use opencode_session_runtime::legacy::ports::LegacyAgents;
use opencode_session_runtime::legacy::types::{AgentInfo, LegacyError, WithParts};
use opencode_session_runtime::title::{
    TitleContext, TitleModel, TitleRequest, Titles, UnwiredTitleModel, clean_title,
};
use serde_json::json;
use support::{SESSION, setup};

struct TitleAgents;

#[async_trait]
impl LegacyAgents for TitleAgents {
    async fn get(&self, name: &str) -> Result<Option<AgentInfo>, LegacyError> {
        Ok((name == "title").then(|| support::agent("title", "primary")))
    }
    async fn list(&self) -> Result<Vec<AgentInfo>, LegacyError> {
        Ok(vec![])
    }
    async fn default_info(&self) -> Result<AgentInfo, LegacyError> {
        Ok(support::agent("build", "primary"))
    }
}

#[derive(Default)]
struct ScriptedTitle {
    text: String,
    requests: Mutex<Vec<TitleRequest>>,
}

#[async_trait]
impl TitleModel for ScriptedTitle {
    async fn stream(&self, request: TitleRequest) -> Result<LlmEventStream, LegacyError> {
        self.requests.lock().unwrap().push(request);
        let event = LlmEvent::TextDelta {
            id: "t".into(),
            text: self.text.clone(),
            provider_metadata: None,
        };
        Ok(Box::pin(futures::stream::iter(vec![Ok(event)])))
    }
}

fn session(title: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_value(json!({ "id": SESSION, "title": title })).unwrap()
}

fn user(text: &str, synthetic: bool) -> WithParts {
    serde_json::from_value(json!({ "info": { "id": "msg_u", "role": "user" }, "parts": [{ "type": "text", "text": text, "synthetic": synthetic }] })).unwrap()
}

#[test]
fn clean_title_strips_think_and_truncates() {
    assert_eq!(
        clean_title("<think>hmm</think>\n\n  Fix login bug  \nmore").as_deref(),
        Some("Fix login bug")
    );
    assert_eq!(clean_title("   \n  ").as_deref(), None);
    let long = "x".repeat(120);
    assert_eq!(
        clean_title(&long).unwrap(),
        format!("{}...", "x".repeat(97))
    );
}

#[tokio::test]
async fn generates_and_persists_title_for_first_real_message() {
    let ctx = setup().await;
    let model = Arc::new(ScriptedTitle {
        text: "<think>a</think>Debug the parser".into(),
        ..Default::default()
    });
    let titles = Titles {
        store: ctx.legacy.store().clone(),
        agents: Arc::new(TitleAgents),
        model: model.clone(),
    };
    let history = vec![user("hello", false)];
    let title = titles
        .ensure(
            &session("New session - 2025-01-02T03:04:05.678Z"),
            &history,
            "test",
            "model",
        )
        .await
        .unwrap();
    assert_eq!(title.as_deref(), Some("Debug the parser"));
    assert_eq!(
        ctx.legacy.store().get(SESSION).await.unwrap()["title"],
        "Debug the parser"
    );
    let request = &model.requests.lock().unwrap()[0];
    assert_eq!(
        request.instruction,
        "Generate a title for this conversation:\n"
    );
    assert_eq!(request.context, TitleContext::History(history));
    assert_eq!(
        (request.provider_id.as_str(), request.model_id.as_str()),
        ("test", "model")
    );
}

#[tokio::test]
async fn ineligible_sessions_never_call_the_model() {
    let ctx = setup().await;
    let model = Arc::new(ScriptedTitle {
        text: "t".into(),
        ..Default::default()
    });
    let titles = Titles {
        store: ctx.legacy.store().clone(),
        agents: Arc::new(TitleAgents),
        model: model.clone(),
    };
    let default = session("New session - 2025-01-02T03:04:05.678Z");
    assert_eq!(
        titles
            .ensure(&session("Custom"), &[user("a", false)], "p", "m")
            .await
            .unwrap(),
        None
    );
    let mut child = default.clone();
    child.insert("parentID".into(), json!("ses_parent"));
    assert_eq!(
        titles
            .ensure(&child, &[user("a", false)], "p", "m")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        titles
            .ensure(&default, &[user("a", true)], "p", "m")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        titles
            .ensure(&default, &[user("a", false), user("b", false)], "p", "m")
            .await
            .unwrap(),
        None
    );
    assert!(model.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn subtask_only_first_message_sends_joined_prompts() {
    let ctx = setup().await;
    let model = Arc::new(ScriptedTitle {
        text: "T".into(),
        ..Default::default()
    });
    let titles = Titles {
        store: ctx.legacy.store().clone(),
        agents: Arc::new(TitleAgents),
        model: model.clone(),
    };
    let first: WithParts = serde_json::from_value(
        json!({ "info": { "id": "msg_u", "role": "user" }, "parts": [
        { "type": "subtask", "prompt": "one" }, { "type": "subtask", "prompt": "two" },
    ] }),
    )
    .unwrap();
    titles
        .ensure(
            &session("Child session - 2025-01-02T03:04:05.678Z"),
            &[first],
            "p",
            "m",
        )
        .await
        .unwrap();
    assert_eq!(
        model.requests.lock().unwrap()[0].context,
        TitleContext::Subtasks("one\ntwo".into())
    );
}

#[tokio::test]
async fn unwired_title_model_fails_typed() {
    let ctx = setup().await;
    let titles = Titles {
        store: ctx.legacy.store().clone(),
        agents: Arc::new(TitleAgents),
        model: Arc::new(UnwiredTitleModel),
    };
    let error = titles
        .ensure(
            &session("New session - 2025-01-02T03:04:05.678Z"),
            &[user("a", false)],
            "p",
            "m",
        )
        .await
        .unwrap_err();
    assert!(matches!(error, LegacyError::NotYetPorted(_)));
}
