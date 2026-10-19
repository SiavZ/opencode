//! Purpose: tests for the canonical session cache. Ports the V2-relevant cases of
//! `packages/app/src/context/server-session.test.ts:1051-1642` (live updates during refresh,
//! removal not restored by stale pages, removals during history prepend, incomplete refresh
//! boundaries, pinned sessions under cache pressure) and `needsOlderTurnRoot`. Fixture-based:
//! fetch timing is simulated with `begin_load`/`apply_page`.
//! Scope: integration tests through `opencode_view_model::session_cache`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! |-- view-model/src/projection/mod.rs
//! |-- view-model/src/session_cache.rs
//! |-- view-model/src/source.rs
//! `-- view-model/src/timeline/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: no network; pages are constructed in memory.
//! Contract tests: self

use opencode_view_model::projection::RenderMessage;
use opencode_view_model::session_cache::{
    Page, PageMode, SessionCache, needs_older_turn_root, pick_evictions,
};
use opencode_view_model::source::{ModelRef, SpanTime};
use opencode_view_model::timeline::SessionStatus;
use opencode_view_model::{SessionEvent, SessionMessage};
use serde_json::{Value, json};
use std::collections::HashSet;

fn user(id: &str, created: i64, text: &str) -> SessionMessage {
    serde_json::from_value(
        json!({ "id": id, "type": "user", "text": text, "time": { "created": created } }),
    )
    .unwrap()
}

fn event(kind: &str, data: Value) -> SessionEvent {
    serde_json::from_value(json!({ "id": "evt_x", "created": 9, "type": kind, "data": data }))
        .unwrap()
}

fn page(source: Vec<SessionMessage>, complete: bool) -> Page {
    Page {
        source,
        cursor: (!complete).then(|| "older".to_owned()),
        complete,
    }
}

fn ids(cache: &SessionCache, s: &str) -> Vec<String> {
    cache.source(s).iter().map(|m| m.id().to_owned()).collect()
}

#[test]
fn preserves_live_updates_during_a_forced_refresh() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_1", 1, "u")], true),
    );
    c.begin_load("child");
    // Live step + streamed text arrive while the refresh is pending.
    let base = json!({ "sessionID": "child", "assistantMessageID": "msg_2", "ordinal": 0 });
    c.apply_event(&event("session.step.started", json!({ "sessionID": "child", "assistantMessageID": "msg_2", "agent": "build", "model": { "id": "m", "providerID": "p" } })));
    c.apply_event(&event("session.text.started", base.clone()));
    let mut delta = base.clone();
    delta["delta"] = json!("live");
    c.apply_event(&event("session.text.delta", delta));
    // Stale page does not contain the live assistant.
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_1", 1, "u")], true),
    );
    assert_eq!(ids(&c, "child"), vec!["msg_1", "msg_2"]);
    let a = c.source("child")[1].as_assistant().unwrap();
    assert_eq!(
        serde_json::to_value(&a.content).unwrap(),
        json!([{ "type": "text", "text": "live" }])
    );
}

#[test]
fn does_not_restore_a_completed_message_removal_from_a_stale_refresh() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_1", 1, "u")], true),
    );
    c.remove_message("child", "msg_1");
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_1", 1, "u")], true),
    );
    assert!(ids(&c, "child").is_empty());
}

#[test]
fn preserves_removals_during_history_prepend() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("message-2", 2, "u")], false),
    );
    c.begin_load("child");
    c.remove_message("child", "message-1");
    c.apply_page(
        "child",
        PageMode::Older,
        page(vec![user("message-1", 1, "u")], true),
    );
    assert_eq!(ids(&c, "child"), vec!["message-2"]);
}

#[test]
fn preserves_loaded_history_during_an_incomplete_refresh() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(
            vec![user("message-1", 1, "a"), user("message-2", 2, "b")],
            false,
        ),
    );
    c.apply_page(
        "child",
        PageMode::Latest,
        page(
            vec![user("message-2", 2, "b"), user("message-3", 3, "c")],
            false,
        ),
    );
    assert_eq!(
        ids(&c, "child"),
        vec!["message-1", "message-2", "message-3"]
    );
}

#[test]
fn drops_stale_recent_messages_omitted_by_an_incomplete_refresh() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(
            vec![user("message-4", 4, "a"), user("message-5", 5, "b")],
            false,
        ),
    );
    c.apply_page(
        "child",
        PageMode::Latest,
        page(
            vec![user("message-3", 3, "c"), user("message-4", 4, "a")],
            false,
        ),
    );
    assert_eq!(ids(&c, "child"), vec!["message-3", "message-4"]);
}

#[test]
fn uses_message_creation_time_for_incomplete_refresh_boundaries() {
    let mut c = SessionCache::default();
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_z", 1, "a"), user("msg_a", 3, "b")], false),
    );
    c.apply_page(
        "child",
        PageMode::Latest,
        page(vec![user("msg_m", 2, "c")], false),
    );
    assert_eq!(ids(&c, "child"), vec!["msg_z", "msg_m"]);
}

#[test]
fn preserves_pinned_session_content_under_server_wide_cache_pressure() {
    let mut c = SessionCache::default();
    c.pin("active");
    c.add_optimistic(
        "active",
        RenderMessage {
            id: "message".into(),
            time: SpanTime {
                created: 1,
                completed: None,
            },
            agent: "agent".into(),
            model: ModelRef {
                id: "model".into(),
                provider_id: "provider".into(),
                variant: None,
            },
            role: opencode_view_model::projection::Role::User,
            summary_diffs: vec![],
        },
    );
    for i in 0..50 {
        c.set_status(&format!("session-{i}"), SessionStatus::Idle);
    }
    let active = c.get("active").unwrap();
    assert_eq!(
        active
            .optimistic
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["message"]
    );
    assert!(c.get("session-0").is_none());
    assert!(c.get("session-49").is_some());
}

#[test]
fn missed_admission_requests_hydration_and_hydrate_confirms_optimistic() {
    let mut c = SessionCache::default();
    let applied = c
        .apply_event(&event(
            "session.input.promoted",
            json!({ "sessionID": "s", "inputID": "msg_u" }),
        ))
        .unwrap();
    assert_eq!(applied.hydrate.as_deref(), Some("msg_u"));
    c.add_optimistic(
        "s",
        RenderMessage {
            id: "msg_u".into(),
            time: SpanTime::default(),
            agent: String::new(),
            model: ModelRef {
                id: String::new(),
                provider_id: String::new(),
                variant: None,
            },
            role: opencode_view_model::projection::Role::User,
            summary_diffs: vec![],
        },
    );
    c.hydrate("s", user("msg_u", 5, "hello"));
    assert_eq!(ids(&c, "s"), vec!["msg_u"]);
    assert!(c.get("s").unwrap().optimistic.is_empty());
}

#[test]
fn execution_events_drive_status() {
    let mut c = SessionCache::default();
    c.apply_event(&event(
        "session.execution.started",
        json!({ "sessionID": "s" }),
    ));
    assert_eq!(c.get("s").unwrap().status, Some(SessionStatus::Busy));
    c.apply_event(&event(
        "session.execution.succeeded",
        json!({ "sessionID": "s" }),
    ));
    assert_eq!(c.get("s").unwrap().status, Some(SessionStatus::Idle));
}

#[test]
fn pick_evictions_keeps_recent_and_preserved() {
    let mut seen: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
    let preserve: HashSet<String> = ["a".to_string()].into();
    let stale = pick_evictions(&mut seen, "b", 2, &preserve);
    // Baseline loop: size 4 > 2, skip preserved "a", evict "c", 3 > 2 evict "d".
    assert_eq!(stale, vec!["c".to_string(), "d".to_string()]);
    assert_eq!(seen, vec!["a", "b"]);
}

#[test]
fn older_turn_root_needed_when_page_starts_with_assistant() {
    let assistant: SessionMessage =
        serde_json::from_value(json!({ "id": "msg_a", "type": "assistant", "agent": "b",
        "model": { "id": "m", "providerID": "p" }, "content": [], "time": { "created": 2 } }))
        .unwrap();
    let agent: SessionMessage = serde_json::from_value(
        json!({ "id": "msg_s", "type": "agent-switched", "agent": "b", "time": { "created": 1 } }),
    )
    .unwrap();
    assert!(needs_older_turn_root(&[agent.clone(), assistant.clone()]));
    assert!(!needs_older_turn_root(&[user("msg_u", 1, "x"), assistant]));
    assert!(!needs_older_turn_root(&[agent]));
}
