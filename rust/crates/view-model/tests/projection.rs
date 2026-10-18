//! Purpose: port of `packages/app/src/utils/session-message.test.ts` (4 cases). Fixture-based.
//! Scope: integration tests through `opencode_view_model::projection::project`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/mod.rs
//! `-- view-model/src/source.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are the baseline test JSON decoded through serde.
//! Contract tests: self

use opencode_view_model::SessionMessage;
use opencode_view_model::projection::{PartKind, Role, ToolStatus, project};
use serde_json::{Value, json};

fn source(value: Value) -> Vec<SessionMessage> {
    serde_json::from_value(value).expect("fixture decodes")
}

fn ids(parts: &[opencode_view_model::projection::RenderPart]) -> Vec<&str> {
    parts.iter().map(|p| p.id.as_str()).collect()
}

#[test]
fn projects_current_turns_into_stable_legacy_rendering_records() {
    let src = source(json!([
        { "id": "msg_1", "type": "agent-switched", "agent": "build", "time": { "created": 1 } },
        { "id": "msg_2", "type": "model-switched", "model": { "id": "claude", "providerID": "anthropic", "variant": "high" }, "time": { "created": 2 } },
        { "id": "msg_3", "type": "user", "text": "inspect @src/client.ts",
          "files": [
            { "data": "aGVsbG8=", "mime": "text/plain", "name": "note.txt", "source": { "type": "inline" } },
            { "data": "ZXhwb3J0IHt9", "mime": "text/plain", "name": "client.ts", "source": { "type": "inline" },
              "mention": { "text": "@src/client.ts", "start": 8, "end": 22 } }
          ],
          "agents": [{ "name": "review", "mention": { "text": "@review", "start": 0, "end": 7 } }],
          "time": { "created": 3 } },
        { "id": "msg_4", "type": "assistant", "agent": "build", "model": { "id": "claude", "providerID": "anthropic", "variant": "high" },
          "content": [
            { "type": "reasoning", "text": "Thinking", "time": { "created": 4, "completed": 5 } },
            { "type": "text", "text": "Result" },
            { "type": "tool", "id": "call_1", "name": "read",
              "state": { "status": "completed", "input": { "filePath": "note.txt" }, "metadata": { "title": "note.txt" },
                         "content": [{ "type": "text", "text": "hello" }] },
              "time": { "created": 5, "ran": 6, "completed": 7 } }
          ],
          "cost": 0.1, "tokens": { "input": 10, "output": 5, "reasoning": 2, "cache": { "read": 1, "write": 0 } },
          "time": { "created": 4, "completed": 7 } },
        { "id": "msg_5", "type": "compaction", "status": "completed", "reason": "auto", "summary": "summary", "recent": "recent", "time": { "created": 8 } }
    ]));
    let r = project(&src);
    assert_eq!(r.messages.len(), 2);
    let user = &r.messages[0];
    assert_eq!(user.id, "msg_3");
    assert!(user.is_user());
    assert_eq!(user.agent, "build");
    assert_eq!(
        (
            user.model.provider_id.as_str(),
            user.model.id.as_str(),
            user.model.variant.as_deref()
        ),
        ("anthropic", "claude", Some("high"))
    );
    let a = &r.messages[1];
    assert_eq!(a.id, "msg_4");
    assert_eq!(a.parent_id(), Some("msg_3"));
    assert!(matches!(a.role, Role::Assistant { cost, .. } if cost == 0.1));
    assert_eq!(
        ids(r.parts_of("msg_3")),
        vec![
            "msg_3:text:0",
            "msg_3:file:0",
            "msg_3:file:1",
            "msg_3:agent:0",
            "msg_5:compaction"
        ]
    );
    assert!(
        matches!(&r.parts_of("msg_3")[2].kind, PartKind::File { mention_path: Some(p), .. } if p == "src/client.ts")
    );
    assert!(
        matches!(&r.parts_of("msg_3")[1].kind, PartKind::File { url, .. } if url == "data:text/plain;base64,aGVsbG8=")
    );
    assert_eq!(
        ids(r.parts_of("msg_4")),
        vec!["msg_4:reasoning:0", "msg_4:text:0", "call_1"]
    );
    let PartKind::Tool { tool, status, .. } = &r.parts_of("msg_4")[2].kind else {
        panic!("tool")
    };
    assert_eq!(tool, "read");
    assert!(matches!(status, ToolStatus::Completed { output, .. } if output == "hello"));
}

#[test]
fn does_not_invent_a_parent_for_an_assistant_only_page() {
    let src = source(json!([
        { "id": "msg_2", "type": "assistant", "agent": "build", "model": { "id": "model", "providerID": "provider" },
          "content": [{ "type": "text", "text": "orphan" }], "time": { "created": 2 } }
    ]));
    assert!(project(&src).messages.is_empty());
}

#[test]
fn projects_a_current_shell_message_into_a_renderable_standalone_turn() {
    let src = source(json!([
        { "id": "msg_shell", "type": "shell", "shellID": "shell_1", "command": "printf hello", "status": "exited", "exit": 0,
          "output": { "output": "hello", "cursor": 5, "size": 5, "truncated": false }, "time": { "created": 1, "completed": 2 } }
    ]));
    let r = project(&src);
    assert_eq!(
        r.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["msg_shell", "msg_shell:assistant"]
    );
    assert!(r.messages[0].is_user());
    assert_eq!(r.messages[1].parent_id(), Some("msg_shell"));
    assert!(
        matches!(&r.parts_of("msg_shell")[0].kind, PartKind::Text { text, .. } if text == "printf hello")
    );
    let PartKind::Tool {
        tool,
        input,
        status,
        ..
    } = &r.parts_of("msg_shell:assistant")[0].kind
    else {
        panic!()
    };
    assert_eq!(tool, "bash");
    assert_eq!(
        Value::Object(input.clone()),
        json!({ "command": "printf hello" })
    );
    assert!(
        matches!(status, ToolStatus::Completed { output, title, .. } if output == "hello" && title == "Shell")
    );
}

#[test]
fn adapts_current_edit_fields_for_the_legacy_edit_renderer() {
    let src = source(json!([
        { "id": "msg_user", "type": "user", "text": "edit it", "time": { "created": 1 } },
        { "id": "msg_assistant", "type": "assistant", "agent": "build", "model": { "id": "model", "providerID": "provider" },
          "content": [{ "type": "tool", "id": "call_edit", "name": "edit",
            "state": { "status": "completed", "input": { "path": "/repo/README.md", "oldString": "old", "newString": "new" },
              "content": [{ "type": "text", "text": "Edited file successfully" }],
              "metadata": { "files": [{ "file": "README.md", "patch": "@@ -1 +1 @@\n-old\n+new", "additions": 1, "deletions": 1, "status": "modified" }], "replacements": 1 } },
            "time": { "created": 2, "ran": 3, "completed": 4 } }],
          "time": { "created": 2, "completed": 4 } }
    ]));
    let r = project(&src);
    let PartKind::Tool {
        input, metadata, ..
    } = &r.parts_of("msg_assistant")[0].kind
    else {
        panic!()
    };
    assert_eq!(input["filePath"], json!("/repo/README.md"));
    assert_eq!(input["path"], json!("/repo/README.md"));
    assert_eq!(
        metadata["filediff"],
        json!({ "file": "README.md", "patch": "@@ -1 +1 @@\n-old\n+new", "additions": 1, "deletions": 1 })
    );
}
