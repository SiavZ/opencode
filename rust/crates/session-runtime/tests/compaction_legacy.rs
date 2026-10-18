//! Purpose: fixture tests for legacy compaction selection and writes: serialization, completed
//! compaction detection, turn/tail selection, prune thresholds, replay split, create and
//! auto-continue messages, and the typed NotYetPorted summary turn.
//! Scope: test-only; real SQLite + legacy projectors via the legacy support harness.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/compaction.rs
//! |-- session-runtime/tests/support/legacy.rs
//! `-- session-runtime/tests/support/sql.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: transcripts are hand-built baseline-shaped fixtures.
//! Contract tests: session-runtime/tests/compaction_legacy.rs

#[path = "support/sql.rs"]
mod sql;
#[path = "support/legacy.rs"]
mod support;

use opencode_session_runtime::legacy::compaction::{
    LegacyCompaction, completed_compactions, continue_text, prune_candidates, replay_split, select,
    serialize, turns,
};
use opencode_session_runtime::legacy::types::{LegacyError, WithParts};
use serde_json::{Value, json};
use support::{SESSION, setup};

fn msg(value: Value) -> WithParts {
    serde_json::from_value(value).unwrap()
}

fn user(id: &str, text: &str) -> WithParts {
    msg(
        json!({ "info": { "id": id, "role": "user" }, "parts": [{ "type": "text", "text": text }] }),
    )
}

fn tool(id: &str, output: &str) -> WithParts {
    msg(json!({
        "info": { "id": id, "role": "assistant", "parentID": "x" },
        "parts": [{ "id": format!("prt_{id}"), "messageID": id, "sessionID": SESSION, "type": "tool", "tool": "bash", "callID": "c",
            "state": { "status": "completed", "input": {}, "output": output, "title": "", "metadata": {}, "time": { "start": 1, "end": 2 } } }],
    }))
}

#[test]
fn serialize_user_and_assistant() {
    let u = msg(json!({ "info": { "role": "user" }, "parts": [
        { "type": "text", "text": "a" }, { "type": "text", "text": "skip", "ignored": true },
        { "type": "file", "mime": "image/png" },
    ] }));
    assert_eq!(serialize(&u), "[User]: a\n[Attached image/png: file]");
    let mut t = tool("msg_t", "out");
    t.parts[0]["state"]["time"]["compacted"] = json!(5);
    assert_eq!(
        serialize(&t),
        "[Assistant tool call]: bash({})\n[Tool result]: [Old tool result content cleared]"
    );
}

#[test]
fn completed_compactions_pair_summary_assistants() {
    let messages = vec![
        msg(
            json!({ "info": { "id": "msg_c", "role": "user" }, "parts": [{ "type": "compaction", "auto": true }] }),
        ),
        msg(
            json!({ "info": { "id": "msg_s", "role": "assistant", "parentID": "msg_c", "summary": true, "finish": "stop" }, "parts": [{ "type": "text", "text": " sum " }] }),
        ),
        msg(
            json!({ "info": { "id": "msg_e", "role": "assistant", "parentID": "msg_c", "summary": true, "finish": "stop", "error": { "name": "x" } }, "parts": [] }),
        ),
    ];
    let found = completed_compactions(&messages);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].user_index, found[0].assistant_index), (0, 1));
    assert_eq!(found[0].summary.as_deref(), Some("sum"));
}

#[test]
fn select_keeps_whole_recent_turns_or_splits() {
    let messages = vec![
        user("msg_1", "a"),
        tool("msg_2", "x"),
        user("msg_3", "b"),
        tool("msg_4", "y"),
    ];
    assert_eq!(turns(&messages).len(), 2);
    let size = |slice: &[WithParts]| slice.len() as f64 * 10.0;
    let kept = select(&messages, None, 20.0, &size);
    assert_eq!(
        (kept.head_len, kept.tail_start_id.as_deref()),
        (2, Some("msg_3"))
    );
    let split = select(&messages, None, 10.0, &size);
    assert_eq!(
        (split.head_len, split.tail_start_id.as_deref()),
        (3, Some("msg_4"))
    );
    let none = select(&messages, Some(0), 1000.0, &size);
    assert_eq!((none.head_len, none.tail_start_id), (4, None));
    let all = select(&messages, None, 1000.0, &size);
    assert_eq!(
        all.tail_start_id, None,
        "a tail starting at 0 keeps everything in head"
    );
}

#[test]
fn prune_protects_recent_window_and_last_two_turns() {
    let big = "a".repeat(100_000); // 25k tokens each
    let messages = vec![
        user("msg_1", "a"),
        tool("msg_2", &big),
        tool("msg_3", &big),
        tool("msg_4", &big),
        tool("msg_5", &big),
        user("msg_6", "b"),
        tool("msg_7", &big),
        user("msg_8", "c"),
    ];
    let (found, pruned, total) = prune_candidates(&messages);
    // Everything newer than the second-to-last user message (msg_6) is skipped. msg_5 fits the
    // 40k protect window (25k); msg_4 crosses it (50k) and is pruned with msg_3 and msg_2.
    assert_eq!(total, 100_000.0);
    assert_eq!(found, vec![(3, 0), (2, 0), (1, 0)]);
    assert_eq!(pruned, 75_000.0);
}

#[test]
fn replay_split_needs_prior_real_user() {
    let messages = vec![
        user("msg_1", "a"),
        user("msg_2", "b"),
        msg(
            json!({ "info": { "id": "msg_c", "role": "user" }, "parts": [{ "type": "compaction", "auto": true }] }),
        ),
    ];
    assert_eq!(replay_split(&messages, "msg_c"), Some(1));
    assert_eq!(replay_split(&messages[1..], "msg_c"), None);
    assert!(continue_text(true).starts_with("The previous request exceeded"));
    assert_eq!(
        continue_text(false),
        "Continue if you have next steps, or stop and ask for clarification if you are unsure how to proceed."
    );
}

#[tokio::test]
async fn prune_marks_parts_and_create_writes_compaction_request() {
    let ctx = setup().await;
    let store = ctx.legacy.store();
    let big = "a".repeat(100_000);
    for (i, text) in ["first", "second", "third"].iter().enumerate() {
        let id = format!("msg_u{i}");
        store.update_message(&serde_json::from_value(json!({ "id": id, "sessionID": SESSION, "role": "user", "time": { "created": i * 10 } })).unwrap()).await.unwrap();
        store.update_part(&serde_json::from_value(json!({ "id": format!("prt_u{i}"), "messageID": id, "sessionID": SESSION, "type": "text", "text": text })).unwrap()).await.unwrap();
        if i == 0 {
            for j in 0..4 {
                let aid = format!("msg_a{j}");
                store.update_message(&serde_json::from_value(json!({ "id": aid, "sessionID": SESSION, "role": "assistant", "parentID": id, "time": { "created": 1 + j } })).unwrap()).await.unwrap();
                let mut part = tool(&aid, &big).parts.remove(0);
                part["id"] = json!(format!("prt_a{j}"));
                store.update_part(&part).await.unwrap();
            }
        }
    }
    let compaction = LegacyCompaction { store };
    assert_eq!(compaction.prune(SESSION, false).await.unwrap(), 0);
    assert_eq!(compaction.prune(SESSION, true).await.unwrap(), 3);
    let messages = ctx.messages().await;
    let compacted: Vec<bool> = messages
        .iter()
        .filter(|m| m.is_assistant())
        .map(|m| m.parts[0]["state"]["time"].get("compacted").is_some())
        .collect();
    assert_eq!(compacted, vec![true, true, true, false]);

    let created = compaction
        .create(
            SESSION,
            "build",
            json!({ "providerID": "test", "modelID": "model" }),
            true,
            None,
        )
        .await
        .unwrap();
    let last = ctx.messages().await.pop().unwrap();
    assert_eq!(last.info["id"], created["id"]);
    assert_eq!(last.parts[0]["type"], "compaction");
    assert_eq!(last.parts[0]["auto"], true);
    assert!(last.parts[0].get("overflow").is_none());
    assert!(matches!(
        compaction.process().await,
        Err(LegacyError::NotYetPorted(_))
    ));
}
