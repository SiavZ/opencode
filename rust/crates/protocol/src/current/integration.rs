//! Purpose: integration and credential groups
//! (`packages/protocol/src/groups/{integration,credential}.ts`).
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- ir::Field
//! Used by:
//! rust/crates/
//! `-- protocol/src/current/mod.rs
//! Invariants: all endpoints take `LocationQuery` and carry `Location` group middleware.
//! `integration.get` returns `data: UndefinedOr(Integration.Info)`: an unknown integration
//! encodes as `{"location": ...}` with `data` omitted (JSON drops `undefined`), so the IR marks
//! `data` optional although the baseline OpenAPI rendering lists it as required.
//! Contract tests: protocol/tests/current_registry.rs

use super::located;
use crate::errors::ErrorKind::InvalidRequest;
use crate::ir::{self, array, object, opt, r, req, string};
use crate::operation::{Endpoint, Group, Method, Middleware};

fn op(name: &str, method: Method, path: &str) -> Endpoint {
    Endpoint::new(name, &format!("v2.{name}"), method, path).location_query()
}

fn integration_id() -> Vec<ir::Field> {
    vec![req("integrationID", ir::branded("Integration.ID"))]
}

fn attempt_id() -> Vec<ir::Field> {
    vec![req("attemptID", ir::branded("Integration.AttemptID"))]
}

pub(super) fn integration() -> Group {
    Group::new(
        "server.integration",
        Some("integrations"),
        Some("Integration discovery and authentication routes."),
    )
    .endpoint(
        op("integration.list", Method::Get, "/api/integration")
            .ok(located(array(r("IntegrationInfo"))))
            .doc(
                "List integrations",
                "Retrieve available integrations and their authentication methods.",
            ),
    )
    .endpoint(
        op(
            "integration.get",
            Method::Get,
            "/api/integration/:integrationID",
        )
        .params(integration_id())
        .ok(object(vec![
            req("location", r("LocationInfo")),
            opt("data", r("IntegrationInfo")),
        ]))
        .doc(
            "Get integration",
            "Retrieve one integration and its authentication methods.",
        ),
    )
    .endpoint(
        op(
            "integration.connect.key",
            Method::Post,
            "/api/integration/:integrationID/connect/key",
        )
        .params(integration_id())
        .payload(object(vec![req("key", string()), opt("label", string())]))
        .no_content()
        .err(InvalidRequest)
        .doc(
            "Connect with key",
            "Run a key authentication method and store the resulting credential.",
        ),
    )
    .endpoint(
        op(
            "integration.connect.oauth",
            Method::Post,
            "/api/integration/:integrationID/connect/oauth",
        )
        .params(integration_id())
        .payload(object(vec![
            req("methodID", ir::branded("Integration.MethodID")),
            req("inputs", ir::record(string(), string())),
            opt("label", string()),
        ]))
        .ok(located(r("IntegrationAttempt")))
        .err(InvalidRequest)
        .doc(
            "Begin OAuth connection",
            "Start an OAuth attempt and return the authorization details.",
        ),
    )
    .endpoint(
        op(
            "integration.attempt.status",
            Method::Get,
            "/api/integration/attempt/:attemptID",
        )
        .params(attempt_id())
        .ok(located(r("IntegrationAttemptStatus")))
        .doc(
            "Get OAuth attempt status",
            "Poll the current status of an OAuth attempt.",
        ),
    )
    .endpoint(
        op(
            "integration.attempt.complete",
            Method::Post,
            "/api/integration/attempt/:attemptID/complete",
        )
        .params(attempt_id())
        .payload(object(vec![opt("code", string())]))
        .no_content()
        .err(InvalidRequest)
        .doc(
            "Complete OAuth connection",
            "Complete a code-based OAuth attempt and store the resulting credential.",
        ),
    )
    .endpoint(
        op(
            "integration.attempt.cancel",
            Method::Delete,
            "/api/integration/attempt/:attemptID",
        )
        .params(attempt_id())
        .no_content()
        .doc(
            "Cancel OAuth connection",
            "Cancel an OAuth attempt and release its resources.",
        ),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn credential() -> Group {
    let id = || vec![req("credentialID", ir::branded("Credential.ID"))];
    Group::new("server.credential", None, None)
        .endpoint(
            op(
                "credential.update",
                Method::Patch,
                "/api/credential/:credentialID",
            )
            .params(id())
            .payload(object(vec![req("label", string())]))
            .no_content()
            .doc("Update credential", "Update a stored credential label."),
        )
        .endpoint(
            op(
                "credential.remove",
                Method::Delete,
                "/api/credential/:credentialID",
            )
            .params(id())
            .no_content()
            .doc(
                "Remove credential",
                "Remove a stored integration credential.",
            ),
        )
        .with_middleware(Middleware::Location)
}
