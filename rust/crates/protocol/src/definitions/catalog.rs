//! Purpose: catalog named schemas: provider, model, agent, command, skill, filesystem entry,
//! reference, permission and saved permission
//! (`packages/schema/src/{provider,model,agent,command,skill,filesystem,reference,permission,
//! permission-saved}.ts`).
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
//! Invariants: field order and optionality match the baseline structs, `Schema.Json` record
//! values are `Unknown`.
//! Contract tests: protocol/tests/current_registry.rs

use super::{
    absolute_path, finite, named, permission_id, positive_int, project_id, relative_path,
    session_id, unknown_record,
};
use crate::ir::{
    self, Checks, Definition, Field, Schema, exact, lit, lits, object, r, req, string, union,
};

pub(super) fn definitions() -> Vec<Definition> {
    let mut out = provider();
    out.extend(model());
    out.extend(permission());
    out.extend(misc());
    out
}

fn string_record() -> Schema {
    ir::record(string(), string())
}

fn request_fields() -> Vec<Field> {
    vec![
        req("headers", string_record()),
        req("body", unknown_record()),
    ]
}

fn aisdk_fields() -> Vec<Field> {
    vec![
        req("type", lit("aisdk")),
        req("package", string()),
        exact("url", string()),
        exact("settings", unknown_record()),
    ]
}

fn native_fields() -> Vec<Field> {
    vec![
        req("type", lit("native")),
        exact("url", string()),
        req("settings", unknown_record()),
    ]
}

fn provider() -> Vec<Definition> {
    vec![
        named("Provider.AISDK", object(aisdk_fields())),
        named("Provider.Native", object(native_fields())),
        named(
            "Provider.Api",
            union(vec![r("ProviderAISDK"), r("ProviderNative")]),
        ),
        named("Provider.Request", object(request_fields())),
        named(
            "ProviderV2.Info",
            object(vec![
                req("id", ir::branded("ProviderV2.ID")),
                exact("integrationID", ir::branded("Integration.ID")),
                req("name", string()),
                exact("disabled", ir::boolean()),
                req("api", r("ProviderApi")),
                req("request", r("ProviderRequest")),
            ]),
        ),
    ]
}

fn model_id() -> Schema {
    ir::branded("ModelV2.ID")
}

fn with_id(fields: Vec<Field>) -> Schema {
    let mut all = vec![req("id", model_id())];
    all.extend(fields);
    object(all)
}

fn model() -> Vec<Definition> {
    let mut request = request_fields();
    request.push(exact("variant", string()));
    let mut variant = vec![req("id", ir::branded("VariantID"))];
    variant.extend(request_fields());
    vec![
        named(
            "Model.Capabilities",
            object(vec![
                req("tools", ir::boolean()),
                req("input", ir::array(string())),
                req("output", ir::array(string())),
            ]),
        ),
        named(
            "Model.Cost",
            object(vec![
                exact(
                    "tier",
                    object(vec![req("type", lit("context")), req("size", ir::int())]),
                ),
                req("input", finite()),
                req("output", finite()),
                req(
                    "cache",
                    object(vec![req("read", finite()), req("write", finite())]),
                ),
            ]),
        ),
        named(
            "Model.Api",
            union(vec![with_id(aisdk_fields()), with_id(native_fields())]),
        ),
        named(
            "ModelV2.Info",
            object(vec![
                req("id", model_id()),
                req("providerID", ir::branded("ProviderV2.ID")),
                exact("family", ir::branded("Family")),
                req("name", string()),
                req("api", r("ModelApi")),
                req("capabilities", r("ModelCapabilities")),
                req("request", object(request)),
                req("variants", ir::array(object(variant))),
                req("time", object(vec![req("released", finite())])),
                req("cost", ir::array(r("ModelCost"))),
                req("status", lits(&["alpha", "beta", "deprecated", "active"])),
                req("enabled", ir::boolean()),
                req(
                    "limit",
                    object(vec![
                        req("context", ir::int()),
                        exact("input", ir::int()),
                        req("output", ir::int()),
                    ]),
                ),
            ]),
        ),
    ]
}

fn permission() -> Vec<Definition> {
    vec![
        named("PermissionV2.Effect", lits(&["allow", "deny", "ask"])),
        named(
            "PermissionV2.Rule",
            object(vec![
                req("action", string()),
                req("resource", string()),
                req("effect", r("PermissionV2Effect")),
            ]),
        ),
        named("PermissionV2.Ruleset", ir::array(r("PermissionV2Rule"))),
        // Single-member union collapses to its member in the rendered schema.
        named(
            "PermissionV2.Source",
            object(vec![
                req("type", lit("tool")),
                req("messageID", string()),
                req("callID", string()),
            ]),
        ),
        named(
            "PermissionV2.Request",
            object(vec![
                req("id", permission_id()),
                req("sessionID", session_id()),
                req("action", string()),
                req("resources", ir::array(string())),
                exact("save", ir::array(string())),
                exact("metadata", unknown_record()),
                exact("source", r("PermissionV2Source")),
            ]),
        ),
        named("PermissionV2.Reply", lits(&["once", "always", "reject"])),
        named(
            "PermissionSaved.Info",
            object(vec![
                req("id", ir::branded("PermissionSaved.ID")),
                req("projectID", project_id()),
                req("action", string()),
                req("resource", string()),
            ]),
        ),
    ]
}

fn agent_color() -> Schema {
    union(vec![
        Schema::String {
            checks: Checks {
                pattern: Some("^#[0-9a-fA-F]{6}$".to_string()),
                ..Checks::default()
            },
            brands: Vec::new(),
        },
        lits(&[
            "primary",
            "secondary",
            "accent",
            "success",
            "warning",
            "error",
            "info",
        ]),
    ])
}

fn misc() -> Vec<Definition> {
    vec![
        named("Agent.Color", agent_color()),
        named(
            "AgentV2.Info",
            object(vec![
                req("id", ir::branded("AgentV2.ID")),
                exact("model", r("ModelRef")),
                req("request", r("ProviderRequest")),
                exact("system", string()),
                exact("description", string()),
                req("mode", lits(&["subagent", "primary", "all"])),
                req("hidden", ir::boolean()),
                exact("color", r("AgentColor")),
                exact("steps", positive_int()),
                req("permissions", r("PermissionV2Ruleset")),
            ]),
        ),
        named(
            "CommandV2.Info",
            object(vec![
                req("name", string()),
                req("template", string()),
                exact("description", string()),
                exact("agent", string()),
                exact("model", r("ModelRef")),
                exact("subtask", ir::boolean()),
            ]),
        ),
        named(
            "SkillV2.Info",
            object(vec![
                req("name", string()),
                exact("description", string()),
                exact("slash", ir::boolean()),
                req("location", absolute_path()),
                req("content", string()),
            ]),
        ),
        named(
            "FileSystem.Entry",
            object(vec![
                req("path", relative_path()),
                req("type", lits(&["file", "directory"])),
            ]),
        ),
        named(
            "Reference.LocalSource",
            object(vec![
                req("type", lit("local")),
                req("path", absolute_path()),
                exact("description", string()),
                exact("hidden", ir::boolean()),
            ]),
        ),
        named(
            "Reference.GitSource",
            object(vec![
                req("type", lit("git")),
                req("repository", string()),
                exact("branch", string()),
                exact("description", string()),
                exact("hidden", ir::boolean()),
            ]),
        ),
        named(
            "Reference.Source",
            union(vec![r("ReferenceLocalSource"), r("ReferenceGitSource")]),
        ),
        named(
            "Reference.Info",
            object(vec![
                req("name", string()),
                req("path", absolute_path()),
                exact("description", string()),
                exact("hidden", ir::boolean()),
                req("source", r("ReferenceSource")),
            ]),
        ),
    ]
}
