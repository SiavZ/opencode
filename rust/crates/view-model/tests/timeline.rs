//! Purpose: ports of `packages/app/src/pages/session/timeline/{rows-current,projection,model,summary-diffs}.test.ts`
//! plus grouping/heading/error helpers. Fixture-based. The baseline `rows-current` suite mocks
//! `groupParts` so group keys equal part IDs; these tests run the real grouping, so expected
//! group keys carry the real `part:${messageID}:${partID}` form.
//! Scope: integration tests through `opencode_view_model::timeline` and `projection`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/mod.rs
//! |-- view-model/src/timeline/mod.rs
//! |-- view-model/src/timeline/reconcile.rs
//! `-- view-model/src/timeline/text.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are baseline test JSON decoded through serde.
//! Contract tests: self

use opencode_view_model::SessionMessage;
use opencode_view_model::projection::{RenderMessage, Role, SummaryDiff, project};
use opencode_view_model::source::{ModelRef, SpanTime};
use opencode_view_model::timeline::text::{reasoning_heading, unwrap_error_message};
use opencode_view_model::timeline::{
    PartGroup, PartRef, RowOptions, SessionStatus, TimelineRow, construct_rows, is_timeline_ready,
    reuse_rows, unique_summary_diffs, visible_user_messages,
};
use serde_json::{Value, json};

fn source(value: Value) -> Vec<SessionMessage> {
    serde_json::from_value(value).expect("fixture decodes")
}

fn rows_for(
    src: &[SessionMessage],
    page: &[SessionMessage],
    status: SessionStatus,
    extra: &[RenderMessage],
) -> (Option<String>, Vec<String>) {
    let p = project(src);
    let mut users: Vec<RenderMessage> =
        p.messages.iter().filter(|m| m.is_user()).cloned().collect();
    users.extend_from_slice(extra);
    let r = construct_rows(
        page,
        &p,
        &users,
        &RowOptions {
            show_reasoning: true,
            status,
            inline_comments: true,
        },
    );
    (
        r.active_message_id,
        r.rows.iter().map(TimelineRow::key).collect(),
    )
}

fn assistant(id: &str, created: i64, content: Value) -> Value {
    json!({ "id": id, "type": "assistant", "agent": "build", "model": { "id": "model", "providerID": "provider" },
            "content": content, "time": { "created": created } })
}

#[test]
fn derives_turns_and_tagged_rows_from_chronological_current_messages() {
    let mut a2 = assistant("msg_2", 2, json!([{ "type": "text", "text": "answer" }]));
    a2["time"]["completed"] = json!(3);
    let src = source(json!([
        { "id": "msg_1", "type": "user", "text": "first", "time": { "created": 1 } }, a2,
        { "id": "msg_3", "type": "user", "text": "second", "time": { "created": 4 } },
        assistant("msg_4", 5, json!([{ "type": "reasoning", "text": "working" }]))
    ]));
    let (active, keys) = rows_for(&src, &src, SessionStatus::Busy, &[]);
    assert_eq!(active.as_deref(), Some("msg_3"));
    assert_eq!(
        keys,
        vec![
            "user-message:msg_1",
            "assistant-part:msg_1:part:msg_2:msg_2:text:0",
            "turn-gap:msg_3",
            "user-message:msg_3",
            "assistant-part:msg_3:part:msg_4:msg_4:reasoning:0",
        ]
    );
}

#[test]
fn renders_a_current_shell_message_as_a_standalone_turn() {
    let src = source(
        json!([{ "id": "msg_shell", "type": "shell", "shellID": "shell_1", "command": "pwd", "status": "exited", "exit": 0,
        "output": { "output": "/repo", "cursor": 5, "size": 5, "truncated": false }, "time": { "created": 1, "completed": 2 } }]),
    );
    let (active, keys) = rows_for(&src, &src, SessionStatus::Idle, &[]);
    assert_eq!(active.as_deref(), Some("msg_shell"));
    assert_eq!(
        keys,
        vec![
            "user-message:msg_shell",
            "assistant-part:msg_shell:part:msg_shell:assistant:msg_shell:tool"
        ]
    );
}

#[test]
fn keeps_a_projected_parent_missing_from_the_source_page_before_newer_turns() {
    let src = source(json!([
        { "id": "msg_user_1", "type": "user", "text": "first question", "time": { "created": 1 } },
        assistant("msg_assistant_1", 2, json!([{ "type": "text", "text": "first answer" }])),
        { "id": "msg_user_2", "type": "user", "text": "second question", "time": { "created": 4 } },
        assistant("msg_assistant_2", 5, json!([{ "type": "text", "text": "second answer" }]))
    ]));
    let (_, keys) = rows_for(&src, &src[1..], SessionStatus::Idle, &[]);
    assert_eq!(
        keys,
        vec![
            "user-message:msg_user_1",
            "assistant-part:msg_user_1:part:msg_assistant_1:msg_assistant_1:text:0",
            "turn-gap:msg_user_2",
            "user-message:msg_user_2",
            "assistant-part:msg_user_2:part:msg_assistant_2:msg_assistant_2:text:0",
        ]
    );
}

#[test]
fn renders_an_optimistic_user_turn_and_thinking_before_the_protocol_message_arrives() {
    let src = source(
        json!([{ "id": "msg_z", "type": "user", "text": "existing", "time": { "created": 1 } }]),
    );
    let optimistic = RenderMessage {
        id: "msg_a".into(),
        time: SpanTime {
            created: 2,
            completed: None,
        },
        agent: "build".into(),
        model: ModelRef {
            id: "model".into(),
            provider_id: "provider".into(),
            variant: None,
        },
        role: Role::User,
        summary_diffs: vec![],
    };
    let (active, keys) = rows_for(&src, &src, SessionStatus::Busy, &[optimistic]);
    assert_eq!(active.as_deref(), Some("msg_a"));
    assert_eq!(
        keys,
        vec![
            "user-message:msg_z",
            "turn-gap:msg_a",
            "user-message:msg_a",
            "thinking:msg_a"
        ]
    );
}

#[test]
fn removes_a_failed_assistant_error_when_the_turn_continues_streaming() {
    let mut failed = assistant("msg_failed", 2, json!([]));
    failed["error"] = json!({ "type": "ProviderError", "message": "temporary failure" });
    failed["time"]["completed"] = json!(3);
    let src = source(json!([
        { "id": "msg_user", "type": "user", "text": "recover", "time": { "created": 1 } }, failed,
        assistant("msg_recovery", 4, json!([{ "type": "text", "text": "streaming again" }]))
    ]));
    let p = project(&src);
    let users: Vec<_> = p.messages.iter().filter(|m| m.is_user()).cloned().collect();
    let r = construct_rows(
        &src,
        &p,
        &users,
        &RowOptions {
            show_reasoning: true,
            status: SessionStatus::Busy,
            inline_comments: true,
        },
    );
    assert_eq!(
        r.rows.iter().map(TimelineRow::tag).collect::<Vec<_>>(),
        vec!["UserMessage", "AssistantPart"]
    );
}

#[test]
fn error_row_unwraps_provider_json_and_interrupt_renders_divider() {
    let mut failed = assistant("msg_a", 2, json!([{ "type": "text", "text": "partial" }]));
    failed["error"] = json!({ "type": "ProviderError", "message": "Error: {\"error\":{\"type\":\"overloaded\",\"message\":\"busy\"}}" });
    let mut aborted = assistant("msg_b", 2, json!([{ "type": "text", "text": "cut" }]));
    aborted["error"] = json!({ "type": "MessageAbortedError", "message": "aborted" });
    let src = source(json!([
        { "id": "msg_u1", "type": "user", "text": "x", "time": { "created": 1 } }, aborted,
        assistant("msg_c", 3, json!([{ "type": "text", "text": "after" }])),
        { "id": "msg_u2", "type": "user", "text": "y", "time": { "created": 4 } }, failed
    ]));
    let (_, keys) = rows_for(&src, &src, SessionStatus::Idle, &[]);
    assert!(keys.contains(&"turn-divider:msg_u1:interrupted".to_owned()));
    assert_eq!(keys.last().map(String::as_str), Some("error:msg_u2"));
    assert_eq!(
        unwrap_error_message("Error: {\"error\":{\"type\":\"overloaded\",\"message\":\"busy\"}}"),
        "overloaded: busy"
    );
    assert_eq!(unwrap_error_message("plain"), "plain");
}

#[test]
fn groups_adjacent_context_tools_and_hides_todowrite() {
    let tool = |id: &str, name: &str| {
        json!({ "type": "tool", "id": id, "name": name,
        "state": { "status": "completed", "input": {}, "content": [{ "type": "text", "text": "x" }] }, "time": { "created": 2 } })
    };
    let src = source(json!([
        { "id": "msg_u", "type": "user", "text": "x", "time": { "created": 1 } },
        assistant("msg_a", 2, json!([tool("c1", "read"), tool("c2", "grep"), tool("c3", "todowrite"), tool("c4", "bash"), tool("c5", "glob")]))
    ]));
    let (_, keys) = rows_for(&src, &src, SessionStatus::Idle, &[]);
    assert_eq!(
        keys,
        vec![
            "user-message:msg_u",
            "assistant-part:msg_u:context:c1",
            "assistant-part:msg_u:part:msg_a:c4",
            "assistant-part:msg_u:context:c5",
        ]
    );
}

fn context(key: &str, parts: &[&str], user: &str) -> TimelineRow {
    TimelineRow::AssistantPart {
        user_message_id: user.into(),
        group: PartGroup::Context {
            key: key.into(),
            refs: parts
                .iter()
                .map(|p| PartRef {
                    message_id: "assistant-1".into(),
                    part_id: (*p).into(),
                })
                .collect(),
        },
        previous_assistant_part: false,
    }
}

#[test]
fn reuse_timeline_rows_table() {
    let c = |k: &str, p: &[&str]| context(k, p, "user-1");
    let user = TimelineRow::UserMessage {
        user_message_id: "user-1".into(),
        anchor: true,
    };
    type Case<'a> = (
        &'a str,
        Vec<TimelineRow>,
        Vec<TimelineRow>,
        Vec<&'a str>,
        bool,
    );
    let cases: Vec<Case> = vec![
        (
            "reuses an unchanged context group",
            vec![c("context:a", &["a", "b"])],
            vec![c("context:a", &["a", "b"])],
            vec!["context:a"],
            true,
        ),
        (
            "preserves the group key when a member is appended",
            vec![c("context:a", &["a"])],
            vec![c("context:a", &["a", "b"])],
            vec!["context:a"],
            false,
        ),
        (
            "preserves the group key when the first member is removed",
            vec![c("context:a", &["a", "b"])],
            vec![c("context:b", &["b"])],
            vec!["context:a"],
            false,
        ),
        (
            "lets only the natural owner retain an old key after a split",
            vec![c("context:a", &["a", "b"])],
            vec![c("context:a", &["a"]), c("context:b", &["b"])],
            vec!["context:a", "context:b"],
            false,
        ),
        (
            "chooses the earliest prior key when groups merge",
            vec![c("context:a", &["a"]), c("context:b", &["b"])],
            vec![c("context:b", &["b", "a"])],
            vec!["context:a"],
            false,
        ),
        (
            "reserves an old key for its natural owner when two new groups compete",
            vec![c("context:a", &["a", "b"])],
            vec![c("context:b", &["b"]), c("context:a", &["a"])],
            vec!["context:b", "context:a"],
            false,
        ),
        (
            "does not create accidental key collisions",
            vec![c("context:a", &["a", "b", "c"])],
            vec![
                c("context:b", &["b"]),
                c("context:a", &["a"]),
                c("context:c", &["c"]),
            ],
            vec!["context:b", "context:a", "context:c"],
            false,
        ),
    ];
    for (name, previous, rows, expected, unchanged) in cases {
        let r = reuse_rows(&previous, rows);
        let keys: Vec<String> = r.rows.iter().map(TimelineRow::key).collect();
        let want: Vec<String> = expected
            .iter()
            .map(|k| format!("assistant-part:user-1:{k}"))
            .collect();
        assert_eq!(keys, want, "{name}");
        assert_eq!(
            keys.iter().collect::<std::collections::HashSet<_>>().len(),
            keys.len(),
            "{name}"
        );
        assert_eq!(r.unchanged, unchanged, "{name}");
    }
    let cross = reuse_rows(
        &[context("context:a", &["a", "b"], "user-1")],
        vec![context("context:b", &["b"], "user-2")],
    );
    assert_eq!(
        cross.rows[0].key(),
        "assistant-part:user-2:context:b",
        "does not reuse context identity across user messages"
    );
    let ordinary = reuse_rows(std::slice::from_ref(&user), vec![user.clone()]);
    assert!(ordinary.unchanged, "reuses an unaffected ordinary row");
}

fn msg(id: &str, user: bool) -> RenderMessage {
    RenderMessage {
        id: id.into(),
        time: SpanTime::default(),
        agent: String::new(),
        model: ModelRef {
            id: String::new(),
            provider_id: String::new(),
            variant: None,
        },
        role: if user {
            Role::User
        } else {
            Role::Assistant {
                parent_id: String::new(),
                error: None,
                finish: None,
                cost: 0.0,
                tokens: None,
            }
        },
        summary_diffs: vec![],
    }
}

#[test]
fn selects_users_and_applies_the_revert_boundary() {
    let messages = vec![
        msg("msg_z", true),
        msg("msg_a", false),
        msg("msg_b", true),
        msg("msg_c", true),
    ];
    let ids = |v: Vec<&RenderMessage>| v.into_iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    assert_eq!(
        ids(visible_user_messages(&messages, None)),
        vec!["msg_z", "msg_b", "msg_c"]
    );
    assert_eq!(
        ids(visible_user_messages(&messages, Some("msg_b"))),
        vec!["msg_z"]
    );
}

#[test]
fn waits_for_an_assistant_only_load_to_hydrate_its_user_root() {
    assert!(!is_timeline_ready(Some(&[msg("msg_2", false)]), true));
    assert!(is_timeline_ready(
        Some(&[msg("msg_1", true), msg("msg_2", false)]),
        true
    ));
    assert!(is_timeline_ready(Some(&[]), false));
    assert!(!is_timeline_ready(None, false));
}

#[test]
fn unique_summary_diffs_keeps_last_per_file_in_display_order() {
    let d = |f: &str, a: i64| SummaryDiff {
        file: f.into(),
        additions: a,
        deletions: 0,
    };
    assert!(unique_summary_diffs(&[]).is_empty());
    assert_eq!(
        unique_summary_diffs(&[d("alpha.ts", 1), d("beta.ts", 1)]),
        vec![d("alpha.ts", 1), d("beta.ts", 1)]
    );
    assert_eq!(
        unique_summary_diffs(&[
            d("alpha.ts", 1),
            d("beta.ts", 1),
            d("alpha.ts", 2),
            d("charlie.ts", 1),
            d("beta.ts", 2)
        ]),
        vec![d("alpha.ts", 2), d("charlie.ts", 1), d("beta.ts", 2)]
    );
}

#[test]
fn reasoning_heading_precedence() {
    assert_eq!(
        reasoning_heading("intro\n<h2>Plan <b>it</b></h2>").as_deref(),
        Some("Plan  it")
    );
    assert_eq!(
        reasoning_heading("## Reading `files` ##\nbody").as_deref(),
        Some("Reading files")
    );
    assert_eq!(
        reasoning_heading("Title\n=====\n").as_deref(),
        Some("Title")
    );
    assert_eq!(
        reasoning_heading("**Bold [link](http://x)**").as_deref(),
        Some("Bold link")
    );
    assert_eq!(reasoning_heading("just text"), None);
}
