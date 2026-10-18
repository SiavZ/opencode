//! Purpose: session and message groups (`packages/protocol/src/groups/{session,message}.ts`)
//! plus their named response schemas (`SessionsResponse`, `SessionHistory`,
//! `SessionMessagesResponse`).
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/definitions/session.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/current/mod.rs
//! Invariants: `session.list`, `session.create` and `session.active` carry no placement
//! middleware; every sessionID-scoped endpoint carries `SessionLocation`. Query fields use
//! `Schema.optional` (`Optionality::Optional`).
//! Contract tests: protocol/tests/current_registry.rs

use super::{data, positive_from_string};
use crate::definitions::{message_id, project_id, relative_path, session_id, workspace_id};
use crate::errors::ErrorKind::{
    Conflict, InvalidCursor, InvalidRequest, MessageNotFound, ServiceUnavailable, SessionNotFound,
    Unknown,
};
use crate::ir::{self, Definition, Field, Schema, array, def, lits, object, opt, r, req, string};
use crate::operation::{Endpoint, Group, Method, Middleware};

pub(super) fn definitions() -> Vec<Definition> {
    let cursor = |brand: bool| {
        let s = if brand {
            ir::branded("SessionsCursor")
        } else {
            string()
        };
        object(vec![opt("previous", s.clone()), opt("next", s)])
    };
    vec![
        def(
            "SessionsResponse",
            object(vec![
                req("data", array(r("SessionV2Info"))),
                req("cursor", cursor(true)),
            ]),
        ),
        def(
            "SessionHistory",
            object(vec![
                req("data", array(r("SessionDurableEvent"))),
                req("hasMore", ir::boolean()),
            ]),
        ),
        def(
            "SessionMessagesResponse",
            object(vec![
                req("data", array(r("SessionMessage"))),
                req("cursor", cursor(false)),
            ]),
        ),
    ]
}

fn sid() -> Field {
    req("sessionID", session_id())
}

/// The baseline annotates the optional wrapper of `limit`/`order` with a description that the
/// OpenAPI renderer drops ("<What> order for the first page. Use desc for newest first or asc
/// for oldest first."), so the IR carries no field description for them.
fn order() -> Field {
    opt("order", lits(&["asc", "desc"]))
}

/// `SessionsQuery` (identifier `SessionsQuery`).
fn sessions_query() -> Vec<Field> {
    vec![
        opt("workspace", workspace_id()),
        opt("limit", positive_from_string(None)),
        order(),
        opt("search", string()),
        opt("directory", ir::branded("AbsolutePath")),
        opt("project", project_id()),
        opt("subpath", relative_path()),
        opt(
            "cursor",
            ir::described(
                "Opaque pagination cursor returned as cursor.previous or cursor.next in the previous response.",
                ir::branded("SessionsCursor"),
            ),
        ),
    ]
}

/// sessionID-scoped endpoint with `SessionLocation` placement.
fn scoped(name: &str, method: Method, suffix: &str) -> Endpoint {
    Endpoint::new(
        name,
        &format!("v2.{name}"),
        method,
        &format!("/api/session/:sessionID{suffix}"),
    )
    .params(vec![sid()])
    .middleware(Middleware::SessionLocation)
}

fn after_from_string() -> Schema {
    ir::int_from_string(Some(0.0), None)
}

pub(super) fn session() -> Group {
    let s = |n: &str, m: Method, p: &str| Endpoint::new(n, &format!("v2.{n}"), m, p);
    Group::new(
        "server.session",
        Some("sessions"),
        Some("Experimental session routes."),
    )
    .endpoint(
        s("session.list", Method::Get, "/api/session")
            .query(sessions_query())
            .ok(r("SessionsResponse"))
            .err(InvalidCursor)
            .err(InvalidRequest)
            .doc("List sessions", "Retrieve sessions in the requested order. Items keep that order across pages; use cursor.next or cursor.previous to move through the ordered list."),
    )
    .endpoint(
        s("session.create", Method::Post, "/api/session")
            .payload(object(vec![
                opt("id", session_id()),
                opt("agent", ir::branded("AgentV2.ID")),
                opt("model", r("ModelRef")),
                opt("location", r("LocationRef")),
            ]))
            .ok(data(r("SessionV2Info")))
            .doc("Create session", "Create a session at the requested location."),
    )
    .endpoint(
        s("session.active", Method::Get, "/api/session/active")
            .ok(data(ir::record(session_id(), r("SessionActive"))))
            .doc("List active sessions", "Retrieve foreground Session drains currently owned by this OpenCode process. Sessions absent from the result are inactive."),
    )
    .endpoint(
        scoped("session.get", Method::Get, "")
            .ok(data(r("SessionV2Info")))
            .err(SessionNotFound)
            .doc("Get session", "Retrieve a session by ID."),
    )
    .endpoint(
        scoped("session.switchAgent", Method::Post, "/agent")
            .payload(object(vec![req("agent", ir::branded("AgentV2.ID"))]))
            .no_content()
            .err(SessionNotFound)
            .doc("Switch session agent", "Switch the agent used by subsequent provider turns."),
    )
    .endpoint(
        scoped("session.switchModel", Method::Post, "/model")
            .payload(object(vec![req("model", r("ModelRef"))]))
            .no_content()
            .err(SessionNotFound)
            .doc("Switch session model", "Switch the model used by subsequent provider turns."),
    )
    .endpoint(
        scoped("session.prompt", Method::Post, "/prompt")
            .payload(object(vec![
                opt("id", message_id()),
                req("prompt", r("PromptInput")),
                opt("delivery", crate::definitions::delivery()),
                opt("resume", ir::boolean()),
            ]))
            .ok(data(r("SessionInputAdmitted")))
            .err(Conflict)
            .err(SessionNotFound)
            .doc("Send message", "Durably admit one session input and schedule agent-loop execution unless resume is false."),
    )
    .endpoint(
        scoped("session.compact", Method::Post, "/compact")
            .no_content()
            .err(SessionNotFound)
            .err(ServiceUnavailable)
            .doc("Compact session", "Compact a session conversation."),
    )
    .endpoint(
        scoped("session.wait", Method::Post, "/wait")
            .no_content()
            .err(SessionNotFound)
            .err(ServiceUnavailable)
            .doc("Wait for session", "Wait for a session agent loop to become idle."),
    )
    .endpoint(
        scoped("session.revert.stage", Method::Post, "/revert/stage")
            .payload(object(vec![
                req("messageID", message_id()),
                opt("files", ir::boolean()),
            ]))
            .ok(data(r("RevertState")))
            .err(MessageNotFound)
            .err(SessionNotFound)
            .err(Unknown)
            .doc("Stage session revert", "Stage or move a reversible session boundary and optionally apply its file changes."),
    )
    .endpoint(
        scoped("session.revert.clear", Method::Post, "/revert/clear")
            .no_content()
            .err(SessionNotFound)
            .err(Unknown)
            .doc("Clear staged revert", ""),
    )
    .endpoint(
        scoped("session.revert.commit", Method::Post, "/revert/commit")
            .no_content()
            .err(SessionNotFound)
            .doc("Commit staged revert", ""),
    )
    .endpoint(
        scoped("session.context", Method::Get, "/context")
            .ok(data(array(r("SessionMessage"))))
            .err(SessionNotFound)
            .err(Unknown)
            .doc("Get session context", "Retrieve the active context messages for a session (all messages after the last compaction)."),
    )
    .endpoint(
        scoped("session.history", Method::Get, "/history")
            .query(vec![
                opt("limit", positive_from_string(Some(100.0))),
                opt("after", after_from_string()),
            ])
            .ok(r("SessionHistory"))
            .err(SessionNotFound)
            .doc("Get session history", "Read one finite page of public durable Session events after an exclusive aggregate sequence. Newly committed events may appear on later pages."),
    )
    .endpoint(
        scoped("session.events", Method::Get, "/event")
            .query(vec![opt("after", after_from_string())])
            .sse(r("SessionDurableEventStream"))
            .err(SessionNotFound)
            .doc("Subscribe to session events", "Replay durable events after an aggregate sequence, then continue with new durable events."),
    )
    .endpoint(
        scoped("session.interrupt", Method::Post, "/interrupt")
            .no_content()
            .err(SessionNotFound)
            .doc("Interrupt session execution", "Interrupt active execution owned by this OpenCode process. Idle interruption is a no-op."),
    )
    .endpoint(
        scoped("session.message", Method::Get, "/message/:messageID")
            .params(vec![sid(), req("messageID", message_id())])
            .ok(data(r("SessionMessage")))
            .err(SessionNotFound)
            .err(MessageNotFound)
            .doc("Get session message", "Retrieve one projected message owned by the Session."),
    )
}

/// `MessageGroup`, placed with `SessionLocation` group middleware in `makeApi`.
pub(super) fn message() -> Group {
    Group::new(
        "server.message",
        Some("messages"),
        Some("Experimental message routes."),
    )
    .endpoint(
        Endpoint::new(
            "session.messages",
            "v2.session.messages",
            Method::Get,
            "/api/session/:sessionID/message",
        )
        .params(vec![sid()])
        .query(vec![
            opt("limit", ir::int_from_string(Some(1.0), Some(200.0))),
            order(),
            opt(
                "cursor",
                ir::described(
                    "Opaque pagination cursor returned as cursor.previous or cursor.next in the previous response. Do not combine with order.",
                    string(),
                ),
            ),
        ])
        .ok(r("SessionMessagesResponse"))
        .err(InvalidCursor)
        .err(SessionNotFound)
        .err(Unknown)
        .doc("Get session messages", "Retrieve projected messages for a session. Items keep the requested order across pages; use cursor.next or cursor.previous to move through the ordered timeline."),
    )
    .with_middleware(Middleware::SessionLocation)
}
