//! Purpose: projected session message union and its parts
//! (`packages/schema/src/{session-message,llm}.ts`).
//! Scope: static IR construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/definitions/session.rs
//! `-- protocol/src/ir.rs
//! External:
//! `-- ir::array
//! Used by:
//! rust/crates/
//! `-- protocol/src/definitions/mod.rs
//! Invariants: every message variant spreads `Base` (`id`, `metadata?`, `time`) at the position
//! the baseline spreads it, with variant-specific `time` overriding the base key in place.
//! Contract tests: protocol/tests/current_registry.rs

use super::session::tokens;
use super::{finite, message_id, named, relative_path, session_id, unknown_record};
use crate::ir::{self, Definition, Field, Schema, exact, lit, object, r, req, string, union};

pub(super) fn definitions() -> Vec<Definition> {
    let mut out = vec![
        named(
            "LLM.ProviderMetadata",
            ir::record(string(), unknown_record()),
        ),
        named(
            "Tool.TextContent",
            object(vec![req("type", lit("text")), req("text", string())]),
        ),
        named(
            "Tool.FileContent",
            object(vec![
                req("type", lit("file")),
                req("uri", string()),
                req("mime", string()),
                exact("name", string()),
            ]),
        ),
        named(
            "LLM.ToolContent",
            union(vec![r("ToolTextContent"), r("ToolFileContent")]),
        ),
        named(
            "Session.Error.Unknown",
            object(vec![req("type", lit("unknown")), req("message", string())]),
        ),
    ];
    out.extend(tool_states());
    out.extend(assistant_parts());
    out.extend(messages());
    out
}

fn created_time() -> Schema {
    object(vec![req("created", Schema::DateTimeMillis)])
}

fn completed_time() -> Schema {
    object(vec![
        req("created", Schema::DateTimeMillis),
        exact("completed", Schema::DateTimeMillis),
    ])
}

/// `Base` fields followed by `rest`; a `time` key in `rest` replaces the base `time` in place.
fn with_base(rest: Vec<Field>) -> Schema {
    let mut fields = vec![
        req("id", message_id()),
        exact("metadata", unknown_record()),
        req("time", created_time()),
    ];
    for field in rest {
        match fields.iter_mut().find(|f| f.name == field.name) {
            Some(slot) => *slot = field,
            None => fields.push(field),
        }
    }
    object(fields)
}

fn content() -> Field {
    req("content", ir::array(r("LLMToolContent")))
}

fn tool_states() -> Vec<Definition> {
    vec![
        named(
            "Session.Message.ToolState.Pending",
            object(vec![req("status", lit("pending")), req("input", string())]),
        ),
        named(
            "Session.Message.ToolState.Running",
            object(vec![
                req("status", lit("running")),
                req("input", unknown_record()),
                req("structured", unknown_record()),
                content(),
            ]),
        ),
        named(
            "Session.Message.ToolState.Completed",
            object(vec![
                req("status", lit("completed")),
                req("input", unknown_record()),
                exact("attachments", ir::array(r("PromptFileAttachment"))),
                content(),
                exact("outputPaths", ir::array(string())),
                req("structured", unknown_record()),
                exact("result", ir::unknown()),
            ]),
        ),
        named(
            "Session.Message.ToolState.Error",
            object(vec![
                req("status", lit("error")),
                req("input", unknown_record()),
                content(),
                req("structured", unknown_record()),
                req("error", r("SessionErrorUnknown")),
                exact("result", ir::unknown()),
            ]),
        ),
    ]
}

fn assistant_parts() -> Vec<Definition> {
    vec![
        named(
            "Session.Message.Assistant.Text",
            object(vec![
                req("type", lit("text")),
                req("id", string()),
                req("text", string()),
            ]),
        ),
        named(
            "Session.Message.Assistant.Reasoning",
            object(vec![
                req("type", lit("reasoning")),
                req("id", string()),
                req("text", string()),
                exact("providerMetadata", r("LLMProviderMetadata")),
                exact("time", completed_time()),
            ]),
        ),
        named(
            "Session.Message.Assistant.Tool",
            object(vec![
                req("type", lit("tool")),
                req("id", string()),
                req("name", string()),
                exact(
                    "provider",
                    object(vec![
                        req("executed", ir::boolean()),
                        exact("metadata", r("LLMProviderMetadata")),
                        exact("resultMetadata", r("LLMProviderMetadata")),
                    ]),
                ),
                req(
                    "state",
                    union(vec![
                        r("SessionMessageToolStatePending"),
                        r("SessionMessageToolStateRunning"),
                        r("SessionMessageToolStateCompleted"),
                        r("SessionMessageToolStateError"),
                    ]),
                ),
                req(
                    "time",
                    object(vec![
                        req("created", Schema::DateTimeMillis),
                        exact("ran", Schema::DateTimeMillis),
                        exact("completed", Schema::DateTimeMillis),
                        exact("pruned", Schema::DateTimeMillis),
                    ]),
                ),
            ]),
        ),
    ]
}

fn messages() -> Vec<Definition> {
    vec![
        named(
            "Session.Message.AgentSwitched",
            with_base(vec![
                req("type", lit("agent-switched")),
                req("agent", string()),
            ]),
        ),
        named(
            "Session.Message.ModelSwitched",
            with_base(vec![
                req("type", lit("model-switched")),
                req("model", r("ModelRef")),
            ]),
        ),
        named(
            "Session.Message.User",
            with_base(vec![
                req("text", string()),
                exact("files", ir::array(r("PromptFileAttachment"))),
                exact("agents", ir::array(r("PromptAgentAttachment"))),
                req("type", lit("user")),
            ]),
        ),
        named(
            "Session.Message.Synthetic",
            with_base(vec![
                req("sessionID", session_id()),
                req("text", string()),
                req("type", lit("synthetic")),
            ]),
        ),
        named(
            "Session.Message.System",
            with_base(vec![req("type", lit("system")), req("text", string())]),
        ),
        named(
            "Session.Message.Shell",
            with_base(vec![
                req("type", lit("shell")),
                req("callID", string()),
                req("command", string()),
                req("output", string()),
                req("time", completed_time()),
            ]),
        ),
        named("Session.Message.Assistant", assistant()),
        named("Session.Message.Compaction", compaction()),
        named(
            "Session.Message",
            union(vec![
                r("SessionMessageAgentSwitched"),
                r("SessionMessageModelSwitched"),
                r("SessionMessageUser"),
                r("SessionMessageSynthetic"),
                r("SessionMessageSystem"),
                r("SessionMessageShell"),
                r("SessionMessageAssistant"),
                r("SessionMessageCompaction"),
            ]),
        ),
    ]
}

fn assistant() -> Schema {
    with_base(vec![
        req("type", lit("assistant")),
        req("agent", string()),
        req("model", r("ModelRef")),
        req(
            "content",
            ir::array(union(vec![
                r("SessionMessageAssistantText"),
                r("SessionMessageAssistantReasoning"),
                r("SessionMessageAssistantTool"),
            ])),
        ),
        exact(
            "snapshot",
            object(vec![
                exact("start", string()),
                exact("end", string()),
                exact("files", ir::array(relative_path())),
            ]),
        ),
        exact("finish", string()),
        exact("cost", finite()),
        exact("tokens", tokens()),
        exact("error", r("SessionErrorUnknown")),
        req("time", completed_time()),
    ])
}

/// Compaction spreads `Base` last, so its own fields come first.
fn compaction() -> Schema {
    object(vec![
        req("type", lit("compaction")),
        req("reason", ir::lits(&["auto", "manual"])),
        req("summary", string()),
        req("recent", string()),
        req("id", message_id()),
        exact("metadata", unknown_record()),
        req("time", created_time()),
    ])
}
