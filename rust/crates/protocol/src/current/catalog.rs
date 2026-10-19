//! Purpose: location-scoped catalog groups: agent, model, provider, fs, command, skill and
//! reference (`packages/protocol/src/groups/{agent,model,provider,fs,command,skill,reference}.ts`).
//! Scope: static registry construction.
//! Uses:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/definitions/mod.rs
//! |-- protocol/src/errors.rs
//! |-- protocol/src/ir.rs
//! `-- protocol/src/operation.rs
//! External:
//! `-- ir::exact
//! Used by:
//! rust/crates/
//! `-- protocol/src/current/mod.rs
//! Invariants: every endpoint takes `LocationQuery` (deepObject) and carries `Location` group
//! middleware. `fs.read` serves raw bytes from the trailing `*` wildcard path.
//! Contract tests: protocol/tests/current_registry.rs

use super::{located, location_query, positive_from_string};
use crate::definitions::relative_path;
use crate::errors::ErrorKind::{ProviderNotFound, ServiceUnavailable};
use crate::ir::{self, array, lits, opt, r, req, string};
use crate::operation::{Endpoint, Group, Method, Middleware};

fn get(name: &str, path: &str) -> Endpoint {
    Endpoint::new(name, &format!("v2.{name}"), Method::Get, path).location_query()
}

pub(super) fn agent() -> Group {
    Group::new("server.agent", None, None)
        .endpoint(
            get("agent.list", "/api/agent")
                .ok(located(array(r("AgentV2Info"))))
                .doc("List agents", "Retrieve currently registered agents."),
        )
        .with_middleware(Middleware::Location)
}

pub(super) fn model() -> Group {
    Group::new(
        "server.model",
        Some("models"),
        Some("Experimental model routes."),
    )
    .endpoint(
        get("model.list", "/api/model")
            .ok(located(array(r("ModelV2Info"))))
            .err(ServiceUnavailable)
            .doc(
                "List models",
                "Retrieve available models ordered by release date.",
            ),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn provider() -> Group {
    Group::new(
        "server.provider",
        Some("providers"),
        Some("Experimental provider routes."),
    )
    .endpoint(
        get("provider.list", "/api/provider")
            .ok(located(array(r("ProviderV2Info"))))
            .err(ServiceUnavailable)
            .doc(
                "List providers",
                "Retrieve active AI providers so clients can show provider availability and configuration.",
            ),
    )
    .endpoint(
        get("provider.get", "/api/provider/:providerID")
            .params(vec![req("providerID", ir::branded("ProviderV2.ID"))])
            .ok(located(r("ProviderV2Info")))
            .err(ProviderNotFound)
            .err(ServiceUnavailable)
            .doc(
                "Get provider",
                "Retrieve a single AI provider so clients can inspect its availability and endpoint settings.",
            ),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn fs() -> Group {
    let mut list_query = location_query();
    list_query.push(opt("path", relative_path()));
    let mut find_query = location_query();
    find_query.extend([
        req("query", string()),
        // `FileSystem.FindInput.fields.type` keeps the schema package `optional` helper.
        ir::exact("type", lits(&["file", "directory"])),
        opt("limit", positive_from_string(None)),
    ]);
    Group::new(
        "server.fs",
        Some("filesystem"),
        Some("Experimental location-scoped filesystem routes."),
    )
    .endpoint(get("fs.read", "/api/fs/read/*").bytes().doc(
        "Read file",
        "Serve one file relative to the requested location.",
    ))
    .endpoint(
        get("fs.list", "/api/fs/list")
            .query(list_query)
            .ok(located(array(r("FileSystemEntry"))))
            .doc(
                "List directory",
                "List direct children of one directory relative to the requested location.",
            ),
    )
    .endpoint(
        get("fs.find", "/api/fs/find")
            .query(find_query)
            .ok(located(array(r("FileSystemEntry"))))
            .doc(
                "Find files",
                "Find recursively ranked filesystem entries relative to the requested location.",
            ),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn command() -> Group {
    Group::new(
        "server.command",
        Some("commands"),
        Some("Experimental command routes."),
    )
    .endpoint(
        get("command.list", "/api/command")
            .ok(located(array(r("CommandV2Info"))))
            .doc("List commands", "Retrieve currently registered commands."),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn skill() -> Group {
    Group::new(
        "server.skill",
        Some("skills"),
        Some("Experimental skill routes."),
    )
    .endpoint(
        get("skill.list", "/api/skill")
            .ok(located(array(r("SkillV2Info"))))
            .doc("List skills", "Retrieve currently registered skills."),
    )
    .with_middleware(Middleware::Location)
}

pub(super) fn reference() -> Group {
    Group::new(
        "server.reference",
        Some("reference"),
        Some("Location-scoped project references."),
    )
    .endpoint(
        get("reference.list", "/api/reference")
            .ok(located(array(r("ReferenceInfo"))))
            .doc(
                "List references",
                "List references available in the requested location.",
            ),
    )
    .with_middleware(Middleware::Location)
}
