//! Purpose: current Server API registry (`packages/protocol/src/api.ts`, 18 groups) in the
//! baseline registration order, with its named schema definitions.
//! Scope: static registry construction, built once per process.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/catalog.rs
//! |-- protocol/src/current/integration.rs
//! |-- protocol/src/current/permission.rs
//! |-- protocol/src/current/pty.rs
//! |-- protocol/src/current/session.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/events/mod.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- protocol/src/current/catalog.rs
//! |-- protocol/src/current/integration.rs
//! |-- protocol/src/current/permission.rs
//! |-- protocol/src/current/pty.rs
//! |-- protocol/src/current/session.rs
//! `-- protocol/src/lib.rs
//! Invariants: group order health, location, agent, session, message, model, provider,
//! integration, credential, permission, fs, command, skill, event, pty, question, reference,
//! projectCopy. Group middleware (`Location`/`SessionLocation`) precedes `Auth` then
//! `SchemaError` on every endpoint. Endpoint `errors` list only the endpoint-declared errors;
//! middleware errors (401 Auth, 400 SchemaError, 400/404 SessionLocation) are implied by
//! `middleware`.
//! Contract tests: protocol/tests/registry.rs, protocol/tests/current_registry.rs

mod catalog;
mod integration;
mod permission;
mod pty;
mod session;

use crate::errors::ErrorKind;
use crate::ir::{self, Definition, Field, Schema};
use crate::operation::{Api, Endpoint, Group, Method, Middleware};
use std::sync::OnceLock;

/// The current Server API (`HttpApi.make("server")`).
pub fn api() -> &'static Api {
    static API: OnceLock<Api> = OnceLock::new();
    API.get_or_init(build)
}

fn build() -> Api {
    let groups = vec![
        health(),
        location(),
        catalog::agent(),
        session::session(),
        session::message(),
        catalog::model(),
        catalog::provider(),
        integration::integration(),
        integration::credential(),
        permission::permission(),
        catalog::fs(),
        catalog::command(),
        catalog::skill(),
        pty::event(),
        pty::pty(),
        permission::question(),
        catalog::reference(),
        pty::project_copy(),
    ]
    .into_iter()
    .map(|mut g| {
        for e in &mut g.endpoints {
            e.middleware.push(Middleware::Auth);
            e.middleware.push(Middleware::SchemaError);
        }
        g
    })
    .collect();
    let mut definitions: Vec<_> = ErrorKind::ALL.iter().map(|k| k.definition()).collect();
    definitions.extend(crate::definitions::shared());
    definitions.extend(session::definitions());
    definitions.extend(crate::events::definitions());
    definitions.extend(pending_event_definitions(&definitions));
    Api {
        identifier: "server".to_string(),
        title: "opencode HttpApi".to_string(),
        version: "0.0.1".to_string(),
        description: Some("Experimental HttpApi surface for selected instance routes.".to_string()),
        groups,
        definitions,
    }
}

/// Event unions owned by `p06.events-openapi` (`crate::events`). Until that module registers
/// them, they resolve to `Unknown` so refs stay valid; any name it provides wins.
fn pending_event_definitions(existing: &[Definition]) -> Vec<Definition> {
    [
        "V2Event",
        "SessionDurableEvent",
        "SessionDurableEventStream",
    ]
    .into_iter()
    .filter(|name| existing.iter().all(|d| d.name != *name))
    .map(|name| Definition {
        description: Some("Not yet registered by protocol/src/events (p06.events-openapi).".into()),
        ..ir::def(name, ir::unknown())
    })
    .collect()
}

/// `LocationQuery` fields (`packages/protocol/src/groups/location.ts`), rendered deepObject.
pub fn location_query() -> Vec<Field> {
    vec![ir::opt(
        "location",
        ir::object(vec![
            ir::opt("directory", ir::string()),
            ir::opt("workspace", ir::string()),
        ]),
    )]
}

/// `Location.response(data)`: `{ location: Location.Info, data }`.
pub(crate) fn located(data: Schema) -> Schema {
    ir::object(vec![
        ir::req("location", ir::r("LocationInfo")),
        ir::req("data", data),
    ])
}

/// `{ data }` envelope used by non-located responses.
pub(crate) fn data(schema: Schema) -> Schema {
    ir::object(vec![ir::req("data", schema)])
}

/// `Schema.NumberFromString.pipe(Schema.decodeTo(PositiveInt))`, optionally bounded above.
pub(crate) fn positive_from_string(maximum: Option<f64>) -> Schema {
    Schema::Number {
        int: true,
        checks: ir::Checks {
            exclusive_minimum: Some(0.0),
            maximum,
            ..ir::Checks::default()
        },
        from_string: true,
    }
}

impl Endpoint {
    /// `query: LocationQuery` plus `locationQueryOpenApi` (deepObject `location`).
    pub(crate) fn location_query(self) -> Self {
        self.query(location_query()).deep("location")
    }
}

fn health() -> Group {
    Group::new("server.health", None, None).endpoint(
        Endpoint::new("health.get", "v2.health.get", Method::Get, "/api/health")
            .ok(ir::object(vec![ir::req("healthy", ir::lit(true))]))
            .doc(
                "Check server health",
                "Check whether the API server is ready to accept requests.",
            ),
    )
}

fn location() -> Group {
    Group::new("server.location", None, None)
        .endpoint(
            Endpoint::new(
                "location.get",
                "v2.location.get",
                Method::Get,
                "/api/location",
            )
            .location_query()
            .ok(ir::r("LocationInfo"))
            .doc(
                "Get location",
                "Resolve the requested location or the server default location.",
            ),
        )
        .with_middleware(Middleware::Location)
}
