//! Purpose: lower Protocol endpoints to grouped typed Rust request and response operations.
//! Scope: deterministic build-time operation emitter.
//! Uses:
//! |-- rust/xtask/src/codegen/mod.rs
//! `-- rust/xtask/src/codegen/types.rs
//! External:
//! |-- anyhow
//! `-- opencode_protocol
//! Used by:
//! `-- rust/xtask/src/codegen/mod.rs
//! Invariants: all registered endpoints remain present, including TS-generator omissions.
//! Contract tests: xtask/tests/codegen.rs
use super::{header, pascal, snake, types::Types};
use anyhow::{Context, Result, bail};
use opencode_protocol::{
    Api, Endpoint, Group,
    ir::{self, Schema},
    operation::{Middleware, SuccessEncoding},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn api(name: &str, api: &Api, files: &mut BTreeMap<String, String>) -> Result<()> {
    let mut types = Types::new(name, files);
    for definition in &api.definitions {
        types.named(&definition.name, &definition.schema)?;
    }
    let mut root = header(&format!("{name} Protocol client operations")) + "pub mod types;\n";
    for group in &api.groups {
        let group_name = group_name(name, group);
        root.push_str(&format!("pub mod {group_name};\n"));
        let mut module = header(&format!("{} operation group", group.identifier));
        let mut ids = Vec::new();
        let mut names = BTreeSet::new();
        for endpoint in &group.endpoints {
            let method = method_name(name, &group_name, endpoint);
            if !names.insert(method.clone()) {
                bail!(
                    "operation collision: {group_name}.{method} for {}",
                    endpoint.name
                );
            }
            module.push_str(&format!("pub mod {method};\n"));
            let body = operation(name, api, &group_name, &method, endpoint, &mut types)?;
            types
                .files
                .insert(format!("{name}/{group_name}/{method}.rs"), body);
            ids.push(format!(
                "({:?}, {:?}, {:?}, {:?})",
                endpoint.name,
                endpoint.method.as_str(),
                endpoint.path,
                format!("{group_name}::{method}")
            ));
        }
        module.push_str(&format!(
            "pub const OPERATIONS: &[(&str, &str, &str, &str)] = &[\n{}\n];\n",
            ids.iter()
                .map(|id| format!("    {id},"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        types
            .files
            .insert(format!("{name}/{group_name}/mod.rs"), module);
    }
    root.push_str("pub fn operations() -> impl Iterator<Item = &'static (&'static str, &'static str, &'static str, &'static str)> {\n");
    root.push_str(&format!(
        "    [{}].into_iter().flatten()\n}}\n",
        api.groups
            .iter()
            .map(|group| format!("{}::OPERATIONS", group_name(name, group)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    types.files.insert(format!("{name}/mod.rs"), root);
    types.finish();
    Ok(())
}
fn resolved<'a>(api: &'a Api, schema: &'a Schema) -> &'a Schema {
    match schema {
        Schema::Ref { name } => api
            .definition(name)
            .map_or(schema, |definition| &definition.schema),
        Schema::Described { schema, .. } => resolved(api, schema),
        _ => schema,
    }
}
fn operation(
    name: &str,
    api: &Api,
    group: &str,
    method: &str,
    endpoint: &Endpoint,
    types: &mut Types<'_>,
) -> Result<String> {
    let prefix = format!("{}{}", pascal(group), pascal(method));
    let mut params = endpoint.params.clone();
    if endpoint.path.ends_with("/*") {
        params.push(ir::req("wildcard", ir::string()));
    }
    let mut fields = params
        .iter()
        .chain(&endpoint.query)
        .chain(&endpoint.headers)
        .cloned()
        .collect::<Vec<_>>();
    let payload = endpoint
        .payload
        .as_ref()
        .map(|schema| resolved(api, schema));
    let body_names = match payload {
        Some(Schema::Struct { fields: body, .. }) => {
            fields.extend(body.clone());
            body.iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>()
        }
        Some(schema) => {
            fields.push(if endpoint.payload_required {
                ir::req("body", schema.clone())
            } else {
                ir::opt("body", schema.clone())
            });
            vec![]
        }
        None => vec![],
    };
    let mut seen = BTreeSet::new();
    fields.retain(|field| seen.insert(field.name.clone()));
    let input = types.named(&format!("{prefix}Input"), &ir::object(fields))?;
    let success = endpoint
        .successes
        .first()
        .context("endpoint has no success schema")?;
    let wire_schema = success.schema.clone().unwrap_or_else(|| {
        if success.encoding == SuccessEncoding::Bytes {
            Schema::Bytes
        } else {
            ir::object(vec![])
        }
    });
    let unwrapped = matches!(resolved(api, &wire_schema), Schema::Struct { fields, .. } if fields.len() == 1 && fields[0].name == "data")
        && name == "current";
    let output_schema = if unwrapped {
        match resolved(api, &wire_schema) {
            Schema::Struct { fields, .. } => fields[0].schema.clone(),
            _ => unreachable!(),
        }
    } else {
        wire_schema
    };
    let output = if success.encoding == SuccessEncoding::Void {
        "()".into()
    } else {
        types.named(&format!("{prefix}Output"), &output_schema)?
    };
    let errors = errors(endpoint);
    let error = types.named(
        &format!("{prefix}Error"),
        &Schema::Union {
            members: errors.iter().map(|(_, schema)| schema.clone()).collect(),
        },
    )?;
    let mut body = header(&format!(
        "{} {} ({})",
        endpoint.method.as_str(),
        endpoint.path,
        endpoint.name
    ));
    body.push_str(&format!("pub type Input = {input};\npub type Output = {output};\npub type Error = {error};\npub const NAME: &str = {:?};\npub const ENCODING: &str = {:?};\n", endpoint.name, format!("{:?}", success.encoding)));
    body.push_str("pub fn request(input: &Input) -> Result<crate::transport::RequestDescriptor, crate::ClientError> {\n    let value = serde_json::to_value(input).map_err(crate::operation::malformed)?;\n");
    body.push_str(&format!("    crate::operation::descriptor(&value, &crate::operation::Spec {{ method: {:?}, path: {:?}, params: &{:?}, query: &{:?}, headers: &{:?}, body: {}, status: {}, empty: {}, declared: &{:?} }})\n}}\n", endpoint.method.as_str(), endpoint.path, params.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(), endpoint.query.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(), endpoint.headers.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(), match payload { None => "crate::operation::Body::None".into(), Some(Schema::Struct { .. }) => format!("crate::operation::Body::Fields(&{body_names:?})"), Some(_) => "crate::operation::Body::Value(\"body\")".into() }, success.status, success.encoding == SuccessEncoding::Void, errors.iter().map(|(status, _)| *status).collect::<Vec<_>>()));
    if success.encoding == SuccessEncoding::Sse {
        body.push_str("pub fn decode_frame(data: &str) -> Result<Output, crate::ClientError> {\n    serde_json::from_str(data).map_err(crate::operation::malformed)\n}\n");
    }
    if matches!(
        success.encoding,
        SuccessEncoding::Json | SuccessEncoding::Void
    ) {
        body.push_str(&format!("pub fn decode(response: crate::operation::Response) -> Result<Output, crate::errors::ApiError<Error>> {{\n    crate::operation::decode(response, {}, &{:?}, {}, {})\n}}\n", success.status, errors.iter().map(|(status, _)| *status).collect::<Vec<_>>(), success.encoding == SuccessEncoding::Void, unwrapped));
    }
    if success.encoding == SuccessEncoding::Bytes {
        body.push_str(&format!("pub fn decode(response: crate::operation::Response) -> Result<Output, crate::errors::ApiError<Error>> {{\n    crate::operation::decode_bytes(response, {}, &{:?})\n}}\n", success.status, errors.iter().map(|(status, _)| *status).collect::<Vec<_>>()));
    }
    if success.encoding == SuccessEncoding::Json || success.encoding == SuccessEncoding::Void {
        body.push_str("pub fn execute(transport: &impl crate::operation::Transport, input: &Input) -> Result<Output, crate::errors::ApiError<Error>> {\n    let response = transport.execute(request(input)?).map_err(crate::errors::ApiError::Client)?;\n    decode(response)\n}\npub async fn execute_async(transport: &impl crate::operation::AsyncTransport, input: &Input) -> Result<Output, crate::errors::ApiError<Error>> {\n    let response = transport.execute(request(input)?).await.map_err(crate::errors::ApiError::Client)?;\n    decode(response)\n}\n");
    }
    Ok(body)
}
fn errors(endpoint: &Endpoint) -> Vec<(u16, Schema)> {
    let mut errors = endpoint
        .errors
        .iter()
        .map(|error| (error.status, error.schema.clone()))
        .collect::<Vec<_>>();
    for middleware in &endpoint.middleware {
        let extra = match middleware {
            Middleware::Auth => vec![(401, ir::r("UnauthorizedError"))],
            Middleware::SchemaError => vec![(400, ir::r("InvalidRequestError"))],
            Middleware::SessionLocation => vec![
                (400, ir::r("InvalidRequestError")),
                (404, ir::r("SessionNotFoundError")),
            ],
            _ => vec![],
        };
        for error in extra {
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
    }
    errors
}
fn group_name(api: &str, group: &Group) -> String {
    if api == "legacy" {
        return snake(&group.identifier);
    }
    snake(match group.identifier.as_str() {
        "server.agent" => "agents",
        "server.session" => "sessions",
        "server.message" => "messages",
        "server.model" => "models",
        "server.provider" => "providers",
        "server.integration" => "integrations",
        "server.credential" => "credentials",
        "server.permission" => "permissions",
        "server.fs" => "files",
        "server.command" => "commands",
        "server.skill" => "skills",
        "server.event" => "events",
        "server.pty" => "ptys",
        "server.question" => "questions",
        "server.reference" => "references",
        "server.projectCopy" => "projectCopies",
        other => other.strip_prefix("server.").unwrap_or(other),
    })
}
fn method_name(api: &str, group: &str, endpoint: &Endpoint) -> String {
    if api == "legacy"
        && endpoint
            .name
            .split_once('.')
            .is_some_and(|(prefix, _)| snake(prefix) != group)
    {
        return snake(&endpoint.name);
    }
    if api == "current" {
        let alias = match endpoint.name.as_str() {
            "session.messages" => Some("list"),
            "session.revert.stage" => Some("stage"),
            "session.revert.clear" => Some("clear"),
            "session.revert.commit" => Some("commit"),
            "integration.connect.key" => Some("connectKey"),
            "integration.connect.oauth" => Some("connectOauth"),
            "integration.attempt.status" => Some("attemptStatus"),
            "integration.attempt.complete" => Some("attemptComplete"),
            "integration.attempt.cancel" => Some("attemptCancel"),
            "permission.request.list" | "question.request.list" => Some("listRequests"),
            "permission.saved.list" => Some("listSaved"),
            "permission.saved.remove" => Some("removeSaved"),
            _ => None,
        };
        if let Some(alias) = alias {
            return snake(alias);
        }
    }
    snake(
        endpoint
            .name
            .split_once('.')
            .map_or(endpoint.name.as_str(), |(_, tail)| tail),
    )
}
