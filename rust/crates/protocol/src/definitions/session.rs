//! Purpose: Location, Model.Ref, Prompt, Revert and Session named schemas
//! (`packages/schema/src/{location,model,prompt,prompt-input,revert,session,session-input}.ts`).
//! Scope: static IR construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/definitions/mod.rs
//! `-- protocol/src/ir.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/definitions/mod.rs
//! Invariants: field order and optionality match the baseline structs. Times typed
//! `DateTimeUtcFromMillis` are `Schema::DateTimeMillis` (epoch millis on the wire).
//! Contract tests: protocol/tests/current_registry.rs

use super::{
    absolute_path, finite, message_id, named, non_negative_int, project_id, relative_path,
    session_id, workspace_id,
};
use crate::ir::{self, Definition, Schema, exact, object, r, req, string};

pub(super) fn definitions() -> Vec<Definition> {
    vec![
        named(
            "Location.Ref",
            object(vec![
                req("directory", absolute_path()),
                exact("workspaceID", workspace_id()),
            ]),
        ),
        named(
            "Location.Info",
            object(vec![
                req("directory", absolute_path()),
                exact("workspaceID", workspace_id()),
                req(
                    "project",
                    object(vec![
                        req("id", project_id()),
                        req("directory", absolute_path()),
                    ]),
                ),
            ]),
        ),
        named(
            "Model.Ref",
            object(vec![
                req("id", ir::branded("ModelV2.ID")),
                req("providerID", ir::branded("ProviderV2.ID")),
                exact("variant", ir::branded("VariantID")),
            ]),
        ),
        named(
            "Prompt.Source",
            object(vec![
                req("start", finite()),
                req("end", finite()),
                req("text", string()),
            ]),
        ),
        named(
            "Prompt.FileAttachment",
            object(vec![
                req("uri", string()),
                req("mime", string()),
                exact("name", string()),
                exact("description", string()),
                exact("source", r("PromptSource")),
            ]),
        ),
        named(
            "Prompt.AgentAttachment",
            object(vec![
                req("name", string()),
                exact("source", r("PromptSource")),
            ]),
        ),
        named(
            "Prompt",
            object(vec![
                req("text", string()),
                exact("files", ir::array(r("PromptFileAttachment"))),
                exact("agents", ir::array(r("PromptAgentAttachment"))),
            ]),
        ),
        named(
            "PromptInput.FileAttachment",
            object(vec![
                req("uri", string()),
                exact("name", string()),
                exact("description", string()),
                exact("source", r("PromptSource")),
            ]),
        ),
        named(
            "PromptInput",
            object(vec![
                req("text", string()),
                exact("files", ir::array(r("PromptInputFileAttachment"))),
                exact("agents", ir::array(r("PromptAgentAttachment"))),
            ]),
        ),
        named(
            "File.Diff",
            object(vec![
                req("path", relative_path()),
                req("status", ir::lits(&["added", "modified", "deleted"])),
                req("additions", non_negative_int()),
                req("deletions", non_negative_int()),
                req("patch", string()),
            ]),
        ),
        named(
            "Revert.State",
            object(vec![
                req("messageID", message_id()),
                exact("partID", string()),
                exact("snapshot", string()),
                exact("diff", string()),
                exact("files", ir::array(r("FileDiff"))),
            ]),
        ),
        named("SessionV2.Info", session_info()),
        named(
            "SessionActive",
            object(vec![req("type", ir::lit("running"))]),
        ),
        named(
            "SessionInput.Admitted",
            object(vec![
                req("admittedSeq", non_negative_int()),
                req("id", message_id()),
                req("sessionID", session_id()),
                req("prompt", r("Prompt")),
                req("delivery", delivery()),
                req("timeCreated", Schema::DateTimeMillis),
                exact("promotedSeq", non_negative_int()),
            ]),
        ),
    ]
}

/// `SessionInput.Delivery` (`Schema.Literals(["steer", "queue"])`, no identifier).
pub(crate) fn delivery() -> Schema {
    ir::lits(&["steer", "queue"])
}

/// Token usage struct shared by Session and assistant messages.
pub(crate) fn tokens() -> Schema {
    object(vec![
        req("input", finite()),
        req("output", finite()),
        req("reasoning", finite()),
        req(
            "cache",
            object(vec![req("read", finite()), req("write", finite())]),
        ),
    ])
}

fn session_info() -> Schema {
    object(vec![
        req("id", session_id()),
        exact("parentID", session_id()),
        req("projectID", project_id()),
        exact("agent", ir::branded("AgentV2.ID")),
        exact("model", r("ModelRef")),
        req("cost", finite()),
        req("tokens", tokens()),
        req(
            "time",
            object(vec![
                req("created", Schema::DateTimeMillis),
                req("updated", Schema::DateTimeMillis),
                exact("archived", Schema::DateTimeMillis),
            ]),
        ),
        req("title", string()),
        req("location", r("LocationRef")),
        exact("subpath", relative_path()),
        exact("revert", r("RevertState")),
    ])
}
