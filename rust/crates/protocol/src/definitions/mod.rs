//! Purpose: named domain schema definitions (Session, Message, Location, Pty, ...) shared by the
//! current and legacy registries, keyed by baseline OpenAPI component name.
//! Scope: static IR construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/definitions/catalog.rs
//! |-- protocol/src/definitions/integration.rs
//! |-- protocol/src/definitions/message.rs
//! |-- protocol/src/definitions/session.rs
//! `-- protocol/src/ir.rs
//! External:
//! |-- ir::branded
//! |-- ir::id
//! |-- ir::number
//! |-- ir::record
//! |-- ir::string
//! |-- ir::union
//! `-- ir::unknown
//! Used by:
//! rust/crates/
//! |-- protocol/src/current/catalog.rs
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/current/permission.rs
//! |-- protocol/src/current/pty.rs
//! |-- protocol/src/current/session.rs
//! |-- protocol/src/definitions/catalog.rs
//! |-- protocol/src/definitions/integration.rs
//! |-- protocol/src/definitions/message.rs
//! |-- protocol/src/definitions/session.rs
//! `-- protocol/src/lib.rs
//! Invariants: `Definition::name` is the OpenAPI component key (Effect `identifier` with dots
//! removed), `Definition::identifier` keeps the dotted Effect identifier when it differs. Field
//! order is baseline declaration order. The schema package `optional` helper maps to
//! `Optionality::Exact`, `Schema.optional` to `Optionality::Optional`.
//! Contract tests: protocol/tests/current_registry.rs

mod catalog;
mod integration;
mod message;
mod session;

pub(crate) use session::delivery;

use crate::ir::{self, Checks, Definition, Schema};

/// Shared named definitions in registration order. Owner: p06.current-registry.
pub fn shared() -> Vec<Definition> {
    let mut out = Vec::new();
    out.extend(session::definitions());
    out.extend(message::definitions());
    out.extend(catalog::definitions());
    out.extend(integration::definitions());
    out
}

/// Definition keyed by an Effect identifier: component name drops the dots
/// (`SessionV2.Info` -> `SessionV2Info`), the identifier is kept when it differs.
pub(crate) fn named(identifier: &str, schema: Schema) -> Definition {
    let name: String = identifier.chars().filter(|c| *c != '.').collect();
    Definition {
        identifier: (name != identifier).then(|| identifier.to_string()),
        name,
        title: None,
        schema,
        description: None,
    }
}

fn int_with(checks: Checks) -> Schema {
    Schema::Number {
        int: true,
        checks,
        from_string: false,
    }
}

/// `PositiveInt`: integer, exclusive minimum 0.
pub(crate) fn positive_int() -> Schema {
    int_with(Checks {
        exclusive_minimum: Some(0.0),
        ..Checks::default()
    })
}

/// `NonNegativeInt`: integer, minimum 0.
pub(crate) fn non_negative_int() -> Schema {
    int_with(Checks {
        minimum: Some(0.0),
        ..Checks::default()
    })
}

/// `Schema.Finite`: a JSON number (no NaN/Infinity).
pub(crate) fn finite() -> Schema {
    ir::number()
}

/// `Schema.Number`: finite numbers plus the string encodings of the non-finite values.
pub(crate) fn js_number() -> Schema {
    ir::union(vec![
        ir::number(),
        ir::lit("NaN"),
        ir::lit("Infinity"),
        ir::lit("-Infinity"),
    ])
}

pub(crate) fn absolute_path() -> Schema {
    ir::branded("AbsolutePath")
}

pub(crate) fn relative_path() -> Schema {
    ir::branded("RelativePath")
}

/// `Schema.Record(Schema.String, Schema.Unknown)`.
pub(crate) fn unknown_record() -> Schema {
    ir::record(ir::string(), ir::unknown())
}

pub(crate) fn session_id() -> Schema {
    ir::id("^ses", "SessionID")
}

pub(crate) fn message_id() -> Schema {
    ir::id("^msg_", "Session.Message.ID")
}

pub(crate) fn workspace_id() -> Schema {
    ir::id("^wrk", "WorkspaceV2.ID")
}

pub(crate) fn project_id() -> Schema {
    ir::branded("Project.ID")
}

pub(crate) fn permission_id() -> Schema {
    ir::id("^per", "PermissionV2.ID")
}

pub(crate) fn question_id() -> Schema {
    ir::id("^que", "QuestionV2.ID")
}

pub(crate) fn pty_id() -> Schema {
    ir::id("^pty", "PtyID")
}
