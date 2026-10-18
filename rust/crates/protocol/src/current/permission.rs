//! Purpose: permission and question groups, mixing location-scoped and session-scoped endpoints
//! (`packages/protocol/src/groups/{permission,question}.ts`).
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/current/mod.rs
//! Invariants: Effect applies group middleware only to endpoints added before `.middleware`, so
//! the leading endpoints get `Location` and the `/api/session/:sessionID/...` endpoints get only
//! `SessionLocation`. The question group's last `annotateMerge` wins: title `session questions`.
//! Contract tests: protocol/tests/current_registry.rs

use super::{data, located};
use crate::definitions::{permission_id, project_id, question_id, session_id};
use crate::errors::ErrorKind::{PermissionNotFound, QuestionNotFound, SessionNotFound};
use crate::ir::{self, Field, array, exact, object, opt, r, req, string};
use crate::operation::{Endpoint, Group, Method, Middleware};

fn op(name: &str, method: Method, path: &str) -> Endpoint {
    Endpoint::new(name, &format!("v2.{name}"), method, path)
}

fn session_op(name: &str, method: Method, path: &str, params: Vec<Field>) -> Endpoint {
    op(name, method, path)
        .params(params)
        .middleware(Middleware::SessionLocation)
}

fn sid() -> Field {
    req("sessionID", session_id())
}

pub(super) fn permission() -> Group {
    let request = || vec![sid(), req("requestID", permission_id())];
    Group::new(
        "server.permission",
        Some("permissions"),
        Some("Experimental permission routes."),
    )
    .endpoint(
        op(
            "permission.request.list",
            Method::Get,
            "/api/permission/request",
        )
        .location_query()
        .ok(located(array(r("PermissionV2Request"))))
        .doc(
            "List pending permission requests",
            "Retrieve pending permission requests for a location.",
        ),
    )
    .endpoint(
        op(
            "permission.saved.list",
            Method::Get,
            "/api/permission/saved",
        )
        .query(vec![opt("projectID", project_id())])
        .ok(data(array(r("PermissionSavedInfo"))))
        .doc(
            "List saved permissions",
            "Retrieve saved permissions, optionally filtered by project.",
        ),
    )
    .endpoint(
        op(
            "permission.saved.remove",
            Method::Delete,
            "/api/permission/saved/:id",
        )
        .params(vec![req("id", ir::branded("PermissionSaved.ID"))])
        .no_content()
        .doc(
            "Remove saved permission",
            "Remove a saved permission by ID.",
        ),
    )
    .with_middleware(Middleware::Location)
    .endpoint(
        session_op(
            "session.permission.create",
            Method::Post,
            "/api/session/:sessionID/permission",
            vec![sid()],
        )
        .payload(object(vec![
            opt("id", permission_id()),
            req("action", string()),
            req("resources", array(string())),
            exact("save", array(string())),
            exact("metadata", ir::record(string(), ir::unknown())),
            exact("source", r("PermissionV2Source")),
            opt("agent", ir::branded("AgentV2.ID")),
        ]))
        .ok(data(object(vec![
            req("id", permission_id()),
            req("effect", r("PermissionV2Effect")),
        ])))
        .err(SessionNotFound)
        .doc(
            "Create permission request",
            "Evaluate and, when approval is required, create a permission request for a session.",
        ),
    )
    .endpoint(
        session_op(
            "session.permission.list",
            Method::Get,
            "/api/session/:sessionID/permission",
            vec![sid()],
        )
        .ok(data(array(r("PermissionV2Request"))))
        .err(SessionNotFound)
        .doc(
            "List session permission requests",
            "Retrieve pending permission requests owned by a session.",
        ),
    )
    .endpoint(
        session_op(
            "session.permission.get",
            Method::Get,
            "/api/session/:sessionID/permission/:requestID",
            request(),
        )
        .ok(data(r("PermissionV2Request")))
        .err(SessionNotFound)
        .err(PermissionNotFound)
        .doc(
            "Get permission request",
            "Retrieve a pending permission request owned by a session.",
        ),
    )
    .endpoint(
        session_op(
            "session.permission.reply",
            Method::Post,
            "/api/session/:sessionID/permission/:requestID/reply",
            request(),
        )
        .payload(object(vec![
            req("reply", r("PermissionV2Reply")),
            opt("message", string()),
        ]))
        .no_content()
        .err(SessionNotFound)
        .err(PermissionNotFound)
        .doc(
            "Reply to pending permission request",
            "Respond to a pending permission request owned by a session.",
        ),
    )
}

pub(super) fn question() -> Group {
    let request = || vec![sid(), req("requestID", question_id())];
    Group::new(
        "server.question",
        Some("session questions"),
        Some("Experimental session question routes."),
    )
    .endpoint(
        op(
            "question.request.list",
            Method::Get,
            "/api/question/request",
        )
        .location_query()
        .ok(located(array(r("QuestionV2Request"))))
        .doc(
            "List pending question requests",
            "Retrieve pending question requests for a location.",
        ),
    )
    .with_middleware(Middleware::Location)
    .endpoint(
        session_op(
            "session.question.list",
            Method::Get,
            "/api/session/:sessionID/question",
            vec![sid()],
        )
        .ok(data(array(r("QuestionV2Request"))))
        .err(SessionNotFound)
        .doc(
            "List session question requests",
            "Retrieve pending question requests owned by a session.",
        ),
    )
    .endpoint(
        session_op(
            "session.question.reply",
            Method::Post,
            "/api/session/:sessionID/question/:requestID/reply",
            request(),
        )
        .payload(r("QuestionV2Reply"))
        .no_content()
        .err(SessionNotFound)
        .err(QuestionNotFound)
        .doc(
            "Reply to pending question request",
            "Answer a pending question request owned by a session.",
        ),
    )
    .endpoint(
        session_op(
            "session.question.reject",
            Method::Post,
            "/api/session/:sessionID/question/:requestID/reject",
            request(),
        )
        .no_content()
        .err(SessionNotFound)
        .err(QuestionNotFound)
        .doc(
            "Reject pending question request",
            "Reject a pending question request owned by a session.",
        ),
    )
}
