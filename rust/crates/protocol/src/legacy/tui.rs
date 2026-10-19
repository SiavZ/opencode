//! Purpose: legacy TUI event and control endpoint declarations.
//! Scope: static protocol construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/ir.rs
//! |-- protocol/src/legacy/build.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- protocol/src/legacy/mod.rs
//! Invariants: toast duration is positive when supplied; control response accepts arbitrary JSON.
//! Contract tests: protocol/tests/registry.rs

use super::build::{endpoint, group};
use crate::ir::{self, Schema};
use crate::operation::{Group, Method};

pub(super) fn group_definition() -> Group {
    let mut endpoints = vec![
        endpoint("tui.appendPrompt", Method::Post, "/tui/append-prompt").payload(append()),
        endpoint("tui.executeCommand", Method::Post, "/tui/execute-command").payload(command()),
        endpoint("tui.showToast", Method::Post, "/tui/show-toast").payload(toast()),
        endpoint("tui.publish", Method::Post, "/tui/publish").payload(ir::union(vec![
            event("tui.prompt.append", append()),
            event("tui.command.execute", command()),
            event("tui.toast.show", toast()),
            event("tui.session.select", select()),
        ])),
        endpoint("tui.selectSession", Method::Post, "/tui/select-session").payload(select()),
        endpoint("tui.control.next", Method::Get, "/tui/control/next"),
        endpoint(
            "tui.control.response",
            Method::Post,
            "/tui/control/response",
        )
        .payload(ir::unknown()),
    ];
    endpoints.extend(
        [
            ("openHelp", "open-help"),
            ("openSessions", "open-sessions"),
            ("openThemes", "open-themes"),
            ("openModels", "open-models"),
            ("submitPrompt", "submit-prompt"),
            ("clearPrompt", "clear-prompt"),
        ]
        .map(|(name, path)| {
            endpoint(
                &format!("tui.{name}"),
                Method::Post,
                &format!("/tui/{path}"),
            )
        }),
    );
    group("tui", endpoints)
}

fn append() -> Schema {
    ir::object(vec![ir::req("text", ir::string())])
}
fn command() -> Schema {
    ir::object(vec![ir::req("command", ir::string())])
}
fn select() -> Schema {
    ir::object(vec![ir::req("sessionID", ir::id("^ses", "SessionID"))])
}
fn toast() -> Schema {
    ir::object(vec![
        ir::opt("title", ir::string()),
        ir::req("message", ir::string()),
        ir::req(
            "variant",
            ir::lits(&["info", "success", "warning", "error"]),
        ),
        ir::opt("duration", crate::definitions::positive_int()),
    ])
}
fn event(name: &str, properties: Schema) -> Schema {
    ir::object(vec![
        ir::req("type", ir::lit(name)),
        ir::req("properties", properties),
    ])
}
