//! Purpose: event, pty and projectCopy groups
//! (`packages/protocol/src/groups/{event,pty,project-copy}.ts`).
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- ir::Schema
//! Used by:
//! rust/crates/
//! `-- protocol/src/current/mod.rs
//! Invariants: the event group has no placement middleware and streams `V2Event` (owned by
//! `crate::events`). `pty.connect` declares no `LocationQuery`: the raw handler decodes
//! `location[directory]`, `location[workspace]`, `cursor` and `ticket` after the existence check,
//! and upgrades to a WebSocket (OpenAPI renders a JSON boolean 200 plus `x-websocket`). It still
//! carries `Location` because `makeApi` applies `PtyGroup.middleware(location)` to the whole
//! group. `ProjectCopy.StrategyID` is `Schema.Trim` plus non-empty: decode trims, the IR keeps a
//! branded string.
//! `ProjectCopyError` (400) is a `name`/`data` error, not a `_tag` envelope.
//! Contract tests: protocol/tests/current_registry.rs

use super::located;
use crate::definitions::{absolute_path, positive_int, pty_id};
use crate::errors::ErrorKind::{Forbidden, PtyNotFound};
use crate::ir::{self, Field, array, exact, object, r, req, string};
use crate::operation::{Endpoint, Group, Method, Middleware};

fn op(name: &str, method: Method, path: &str) -> Endpoint {
    Endpoint::new(name, &format!("v2.{name}"), method, path)
}

pub(super) fn event() -> Group {
    Group::new(
        "server.event",
        Some("events"),
        Some("Experimental event stream route."),
    )
    .endpoint(
        op("event.subscribe", Method::Get, "/api/event")
            .sse(r("V2Event"))
            .doc(
                "Subscribe to events",
                "Subscribe to native event payloads for the server.",
            ),
    )
}

fn pty_param() -> Vec<Field> {
    vec![req("ptyID", pty_id())]
}

fn create_input() -> ir::Schema {
    object(vec![
        exact("command", string()),
        exact("args", array(string())),
        exact("cwd", string()),
        exact("title", string()),
        exact("env", ir::record(string(), string())),
    ])
}

fn update_input() -> ir::Schema {
    object(vec![
        exact("title", string()),
        exact(
            "size",
            object(vec![
                req("rows", positive_int()),
                req("cols", positive_int()),
            ]),
        ),
    ])
}

pub(super) fn pty() -> Group {
    let located_op = |n: &str, m: Method, p: &str| op(n, m, p).location_query();
    Group::new(
        "server.pty",
        Some("pty"),
        Some("Experimental location-scoped PTY routes."),
    )
    .endpoint(
        located_op("pty.list", Method::Get, "/api/pty")
            .ok(located(array(r("Pty"))))
            .doc(
                "List PTY sessions",
                "List PTY sessions for a location, including exited sessions retained until removal.",
            ),
    )
    .endpoint(
        located_op("pty.create", Method::Post, "/api/pty")
            .payload(create_input())
            .ok(located(r("Pty")))
            .doc(
                "Create PTY session",
                "Create a pseudo-terminal session for a location.",
            ),
    )
    .endpoint(
        located_op("pty.get", Method::Get, "/api/pty/:ptyID")
            .params(pty_param())
            .ok(located(r("Pty")))
            .err(PtyNotFound)
            .doc(
                "Get PTY session",
                "Get one PTY session, including its exit code once exited.",
            ),
    )
    .endpoint(
        located_op("pty.update", Method::Put, "/api/pty/:ptyID")
            .params(pty_param())
            .payload(update_input())
            .ok(located(r("Pty")))
            .err(PtyNotFound)
            .doc(
                "Update PTY session",
                "Update the title or viewport size of one PTY session.",
            ),
    )
    .endpoint(
        located_op("pty.remove", Method::Delete, "/api/pty/:ptyID")
            .params(pty_param())
            .no_content()
            .err(PtyNotFound)
            .doc("Remove PTY session", "Terminate and remove one PTY session."),
    )
    .endpoint(
        located_op("pty.connectToken", Method::Post, "/api/pty/:ptyID/connect-token")
            .params(pty_param())
            .ok(located(r("PtyTicketConnectToken")))
            .err(Forbidden)
            .err(PtyNotFound)
            .doc(
                "Create PTY WebSocket token",
                "Create a short-lived single-use ticket for opening a PTY WebSocket connection.",
            ),
    )
    .endpoint(
        op("pty.connect", Method::Get, "/api/pty/:ptyID/connect")
            .params(pty_param())
            .websocket()
            .err(Forbidden)
            .err(PtyNotFound)
            .doc(
                "Connect to PTY session",
                "Establish a WebSocket connection streaming PTY output and accepting terminal input.",
            ),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn project_copy() -> Group {
    let root = "/experimental/project/:projectID/copy";
    let param = || vec![req("projectID", crate::definitions::project_id())];
    let located_op = |n: &str, m: Method, p: &str| {
        op(n, m, p)
            .location_query()
            .params(param())
            .error(400, r("ProjectCopyError"))
    };
    Group::new(
        "server.projectCopy",
        Some("projectCopy"),
        Some("Project copy management routes."),
    )
    .endpoint(
        located_op("projectCopy.create", Method::Post, root)
            .payload(object(vec![
                req("strategy", ir::branded("ProjectCopy.StrategyID")),
                req("directory", absolute_path()),
                exact("name", string()),
            ]))
            .ok(r("ProjectCopyCopy")),
    )
    .endpoint(
        located_op("projectCopy.remove", Method::Delete, root)
            .payload(object(vec![
                req("directory", absolute_path()),
                req("force", ir::boolean()),
            ]))
            .no_content(),
    )
    .endpoint(
        located_op(
            "projectCopy.refresh",
            Method::Post,
            "/experimental/project/:projectID/copy/refresh",
        )
        .no_content(),
    )
    .with_middleware(Middleware::Location)
}
