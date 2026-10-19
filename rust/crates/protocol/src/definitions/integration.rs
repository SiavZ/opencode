//! Purpose: integration, connection, pty, question and project-copy named schemas
//! (`packages/schema/src/{integration,connection,pty,pty-ticket,question,project-copy}.ts`,
//! `packages/protocol/src/groups/project-copy.ts` `ProjectCopyError`).
//! Scope: static IR construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/definitions/mod.rs
//! `-- protocol/src/ir.rs
//! External:
//! `-- ir::described
//! Used by:
//! rust/crates/
//! `-- protocol/src/definitions/mod.rs
//! Invariants: `Integration.Attempt` times are plain `Schema.Number` (non-finite values encode
//! as strings), unlike the finite numbers used elsewhere.
//! Contract tests: protocol/tests/current_registry.rs

use super::session_id;
use super::{absolute_path, js_number, named, non_negative_int, positive_int, pty_id, question_id};
use crate::ir::{self, Definition, Field, Schema, exact, lit, lits, object, r, req, string, union};

pub(super) fn definitions() -> Vec<Definition> {
    let mut out = integration();
    out.extend(pty());
    out.extend(question());
    out.extend(project_copy());
    out
}

fn when() -> Field {
    exact("when", r("IntegrationWhen"))
}

fn attempt_time() -> Schema {
    object(vec![
        req("created", js_number()),
        req("expires", js_number()),
    ])
}

fn status(name: &str, extra: Vec<Field>) -> Schema {
    let mut fields = vec![req("status", lit(name))];
    fields.extend(extra);
    fields.push(req("time", attempt_time()));
    object(fields)
}

fn integration() -> Vec<Definition> {
    vec![
        named(
            "Integration.When",
            object(vec![
                req("key", string()),
                req("op", lits(&["eq", "neq"])),
                req("value", string()),
            ]),
        ),
        named(
            "Integration.TextPrompt",
            object(vec![
                req("type", lit("text")),
                req("key", string()),
                req("message", string()),
                exact("placeholder", string()),
                when(),
            ]),
        ),
        named(
            "Integration.SelectPrompt",
            object(vec![
                req("type", lit("select")),
                req("key", string()),
                req("message", string()),
                req(
                    "options",
                    ir::array(object(vec![
                        req("label", string()),
                        req("value", string()),
                        exact("hint", string()),
                    ])),
                ),
                when(),
            ]),
        ),
        named(
            "Integration.OAuthMethod",
            object(vec![
                req("id", ir::branded("Integration.MethodID")),
                req("type", lit("oauth")),
                req("label", string()),
                exact(
                    "prompts",
                    ir::array(union(vec![
                        r("IntegrationTextPrompt"),
                        r("IntegrationSelectPrompt"),
                    ])),
                ),
            ]),
        ),
        named(
            "Integration.KeyMethod",
            object(vec![req("type", lit("key")), exact("label", string())]),
        ),
        named(
            "Integration.EnvMethod",
            object(vec![
                req("type", lit("env")),
                req("names", ir::array(string())),
            ]),
        ),
        named(
            "Integration.Method",
            union(vec![
                r("IntegrationOAuthMethod"),
                r("IntegrationKeyMethod"),
                r("IntegrationEnvMethod"),
            ]),
        ),
        named(
            "Connection.CredentialInfo",
            object(vec![
                req("type", lit("credential")),
                req("id", ir::branded("Credential.ID")),
                req("label", string()),
            ]),
        ),
        named(
            "Connection.EnvInfo",
            object(vec![req("type", lit("env")), req("name", string())]),
        ),
        named(
            "Connection.Info",
            union(vec![r("ConnectionCredentialInfo"), r("ConnectionEnvInfo")]),
        ),
        named(
            "Integration.Info",
            object(vec![
                req("id", ir::branded("Integration.ID")),
                req("name", string()),
                req("methods", ir::array(r("IntegrationMethod"))),
                req("connections", ir::array(r("ConnectionInfo"))),
            ]),
        ),
        named(
            "Integration.Attempt",
            object(vec![
                req("attemptID", ir::branded("Integration.AttemptID")),
                req("url", string()),
                req("instructions", string()),
                req("mode", lits(&["auto", "code"])),
                req("time", attempt_time()),
            ]),
        ),
        named(
            "Integration.AttemptStatus",
            union(vec![
                status("pending", vec![]),
                status("complete", vec![]),
                status("failed", vec![req("message", string())]),
                status("expired", vec![]),
            ]),
        ),
    ]
}

fn pty() -> Vec<Definition> {
    vec![
        named(
            "Pty",
            object(vec![
                req("id", pty_id()),
                req("title", string()),
                req("command", string()),
                req("args", ir::array(string())),
                req("cwd", string()),
                req("status", lits(&["running", "exited"])),
                req("pid", non_negative_int()),
                exact("exitCode", non_negative_int()),
            ]),
        ),
        named(
            "PtyTicket.ConnectToken",
            object(vec![
                req("ticket", string()),
                req("expires_in", positive_int()),
            ]),
        ),
    ]
}

fn question() -> Vec<Definition> {
    let doc = |s: Schema, d: &str| ir::described(d, s);
    vec![
        named(
            "QuestionV2.Option",
            object(vec![
                req("label", doc(string(), "Display text (1-5 words, concise)")),
                req("description", doc(string(), "Explanation of choice")),
            ]),
        ),
        named(
            "QuestionV2.Info",
            object(vec![
                req("question", doc(string(), "Complete question")),
                req("header", doc(string(), "Very short label (max 30 chars)")),
                req(
                    "options",
                    doc(ir::array(r("QuestionV2Option")), "Available choices"),
                ),
                // The baseline annotates the optional wrapper ("Allow selecting multiple choices",
                // "Allow typing a custom answer (default: true)"); the rendered schema drops it.
                exact("multiple", ir::boolean()),
                exact("custom", ir::boolean()),
            ]),
        ),
        named(
            "QuestionV2.Tool",
            object(vec![req("messageID", string()), req("callID", string())]),
        ),
        named(
            "QuestionV2.Request",
            object(vec![
                req("id", question_id()),
                req("sessionID", session_id()),
                req(
                    "questions",
                    doc(ir::array(r("QuestionV2Info")), "Questions to ask"),
                ),
                exact("tool", r("QuestionV2Tool")),
            ]),
        ),
        named("QuestionV2.Answer", ir::array(string())),
        named(
            "QuestionV2.Reply",
            object(vec![req(
                "answers",
                doc(
                    ir::array(r("QuestionV2Answer")),
                    "User answers in order of questions (each answer is an array of selected labels)",
                ),
            )]),
        ),
    ]
}

fn project_copy() -> Vec<Definition> {
    vec![
        named(
            "ProjectCopy.Copy",
            object(vec![req("directory", absolute_path())]),
        ),
        named(
            "ProjectCopyError",
            object(vec![
                req("name", lit("ProjectCopyError")),
                req(
                    "data",
                    object(vec![
                        req("message", string()),
                        ir::opt("forceRequired", ir::boolean()),
                    ]),
                ),
            ]),
        ),
    ]
}
