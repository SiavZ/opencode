//! Purpose: OpenAPI 3.x operation generation: one CodeMode tool per representable operation,
//! executed through explicit host capabilities (`HttpClient` for transport, `AuthResolver` for
//! credentials). No ambient network access.
//! Scope: codemode/src/openapi/index.ts and types.ts.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/runtime.rs
//! |-- codemode/src/openapi/schema.rs
//! |-- codemode/src/openapi/security.rs
//! |-- codemode/src/openapi/spec.rs
//! `-- codemode/src/tools/mod.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- serde_json::Value
//! `-- url::Url
//! Used by:
//! rust/crates/
//! `-- codemode/src/lib.rs
//! Invariants: auth is resolved host-side and never model-visible; unrepresentable operations are
//! reported in `skipped` with a reason instead of producing a tool.
//! Contract tests: codemode/tests/openapi.rs

mod naming;
mod responses;
mod runtime;
mod schema;
mod security;
mod spec;

use crate::tools::{
    Definition, ToolCallContext, ToolError, ToolFuture, ToolNode, ToolTree, tool_error,
};
use futures::future::BoxFuture;
use serde_json::{Map, Value as Json};
use std::sync::Arc;

/// The operation identity handed to auth resolution and errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
}

/// A resolved security scheme from `components.securitySchemes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecurityScheme {
    ApiKey { name: String, location: String },
    Http { scheme: String },
    OAuth2,
    OpenIdConnect,
}

/// Credential material from the host. The `apiKey` carrier comes from the scheme definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Bearer { token: String },
    Basic { username: String, password: String },
    ApiKey { value: String },
    Header { name: String, value: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthContext {
    pub name: String,
    pub definition: SecurityScheme,
    pub scopes: Vec<String>,
    pub operation: Operation,
}

/// Resolves credentials per scheme at call time. `Ok(None)` means unavailable (try the next OR
/// alternative); an error aborts the call.
pub trait AuthResolver: Send + Sync {
    fn resolve(
        &self,
        context: AuthContext,
    ) -> BoxFuture<'static, Result<Option<Credential>, ToolError>>;
}

impl<F> AuthResolver for F
where
    F: Fn(AuthContext) -> BoxFuture<'static, Result<Option<Credential>, ToolError>> + Send + Sync,
{
    fn resolve(
        &self,
        context: AuthContext,
    ) -> BoxFuture<'static, Result<Option<Credential>, ToolError>> {
        self(context)
    }
}

/// A request the host transport executes. `query` pairs are appended to `url` in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// The absolute URL including the serialized query string.
    pub fn full_url(&self) -> String {
        match url::Url::parse(&self.url) {
            Ok(mut url) if !self.query.is_empty() => {
                url.query_pairs_mut().extend_pairs(&self.query);
                url.to_string()
            }
            Ok(url) => url.to_string(),
            Err(_) => self.url.clone(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Explicit host HTTP capability. Transport failures are reported as `Err(detail)`; the detail is
/// never model-visible.
pub trait HttpClient: Send + Sync {
    fn execute(&self, request: HttpRequest) -> BoxFuture<'static, Result<HttpResponse, String>>;
}

/// `OpenAPI.fromSpec` options.
#[derive(Clone, Default)]
pub struct Options {
    pub spec: Json,
    /// Overrides all document/path/operation servers.
    pub base_url: Option<String>,
    pub auth: Option<Arc<dyn AuthResolver>>,
    /// Static headers on every request; declared header params may override, auth always wins.
    pub headers: Vec<(String, String)>,
    /// Transport capability. Without it, generated tools refuse to run.
    pub http: Option<Arc<dyn HttpClient>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub method: String,
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Generated {
    pub tools: ToolTree,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Location {
    Path,
    Query,
    Header,
    Body,
}

impl Location {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
            Self::Body => "body",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Simple,
    Form,
    DeepObject,
}

#[derive(Clone, Debug)]
pub(crate) struct InputField {
    pub input_name: String,
    pub name: String,
    pub location: Location,
    pub required: bool,
    pub schema: Json,
    pub style: Option<Style>,
    pub explode: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyMode {
    Object,
    Value,
}

#[derive(Clone, Debug)]
pub(crate) struct Body {
    pub required: bool,
    pub mode: BodyMode,
    pub media_type: String,
}

pub(crate) type SecurityRequirement = Vec<(String, Vec<String>)>;

#[derive(Default)]
pub(crate) struct AppliedAuth {
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
}

pub(crate) struct Plan {
    pub operation: Operation,
    pub url: String,
    pub fields: Vec<InputField>,
    pub body: Option<Body>,
    pub security: Vec<SecurityRequirement>,
    pub schemes: Vec<(String, SecurityScheme)>,
    pub auth: Option<Arc<dyn AuthResolver>>,
    pub headers: Vec<(String, String)>,
}

/// Builds a CodeMode tool subtree from an OpenAPI 3.x document, one tool per operation.
pub fn from_spec(options: &Options) -> Generated {
    let document = &options.spec;
    let schemes = security::security_schemes(document);
    let default_security = security::security_requirements(document.get("security"));
    let definitions = spec::component_definitions(document);
    let paths = document
        .get("paths")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let mut used: Vec<String> = Vec::new();
    let mut namespaces: Vec<String> = Vec::new();
    let mut skipped = Vec::new();
    let mut tools = ToolTree::new();
    for (path, path_value) in crate::tools::render::entries(&paths) {
        let Some(path_item) = path_value.as_object() else {
            continue;
        };
        for (method, operation_value) in path_item {
            let Some(operation) = operation_value
                .as_object()
                .filter(|_| spec::METHODS.contains(&method.as_str()))
            else {
                continue;
            };
            let segments = naming::operation_path(method, path, operation, &used, &namespaces);
            let info = Operation {
                operation_id: spec::non_empty(operation.get("operationId")),
                method: method.to_uppercase(),
                path: path.clone(),
                summary: spec::non_empty(operation.get("summary")),
                description: spec::non_empty(operation.get("description")),
            };
            match plan_operation(
                options,
                document,
                path_item,
                operation,
                &info,
                &definitions,
                &schemes,
                &default_security,
            ) {
                Err(reason) => skipped.push(Skipped {
                    method: info.method.clone(),
                    path: path.clone(),
                    reason,
                }),
                Ok((plan, input, output)) => {
                    used.push(segments.join("."));
                    for end in 1..segments.len() {
                        namespaces.push(segments[..end].join("."));
                    }
                    let description = info
                        .description
                        .clone()
                        .or(info.summary.clone())
                        .unwrap_or_else(|| format!("{} {path}", info.method));
                    let definition =
                        tool_for(plan, options.http.clone(), description, input, output);
                    set_tool(&mut tools, &segments, definition);
                }
            }
        }
    }
    Generated { tools, skipped }
}

#[allow(clippy::too_many_arguments)]
fn plan_operation(
    options: &Options,
    document: &Json,
    path_item: &Map<String, Json>,
    operation: &Map<String, Json>,
    info: &Operation,
    definitions: &Map<String, Json>,
    schemes: &[(String, SecurityScheme)],
    default_security: &spec::Parsed<Vec<SecurityRequirement>>,
) -> spec::Parsed<(Plan, Json, Option<Json>)> {
    let output = responses::operation_output(document, operation, definitions)?;
    let base_url = match &options.base_url {
        Some(base) => naming::validate_base_url(base),
        None if operation.contains_key("servers") => {
            naming::spec_server_url(&Json::Object(operation.clone()))
        }
        None if path_item.contains_key("servers") => {
            naming::spec_server_url(&Json::Object(path_item.clone()))
        }
        None => naming::spec_server_url(document),
    }?;
    let (fields, body) = spec::operation_input(document, path_item, operation)?;
    let security =
        security::operation_security(operation.get("security"), default_security, schemes)?;
    let input = spec::input_schema(&fields, definitions);
    let plan = Plan {
        operation: info.clone(),
        url: format!("{}{}", base_url.trim_end_matches('/'), info.path),
        fields,
        body,
        security,
        schemes: schemes.to_vec(),
        auth: options.auth.clone(),
        headers: options.headers.clone(),
    };
    Ok((plan, input, output))
}

fn tool_for(
    plan: Plan,
    http: Option<Arc<dyn HttpClient>>,
    description: String,
    input: Json,
    output: Option<Json>,
) -> Definition {
    let plan = Arc::new(plan);
    Definition::new(
        description,
        input,
        output.map(Into::into),
        move |input: Json, context: ToolCallContext| -> ToolFuture {
            let (plan, http) = (plan.clone(), http.clone());
            Box::pin(async move {
                let Some(http) = http else {
                    return Err(tool_error(format!(
                        "{} {} requires an HTTP client capability, which the host did not provide.",
                        plan.operation.method, plan.operation.path
                    )));
                };
                invoke(&plan, http.as_ref(), &input, &context).await
            })
        },
    )
}

async fn invoke(
    plan: &Plan,
    http: &dyn HttpClient,
    input: &Json,
    context: &ToolCallContext,
) -> Result<Json, ToolError> {
    let empty = Map::new();
    let value = input.as_object().unwrap_or(&empty);
    let mut request = runtime::build_request(plan, value)?;
    let auth = runtime::resolve_auth(plan).await?;
    for (name, item) in auth.query {
        request.query.retain(|(existing, _)| *existing != name);
        request.query.push((name, item));
    }
    for (name, item) in auth.headers {
        request
            .headers
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
        request.headers.push((name, item));
    }
    if context.cancel.is_cancelled() {
        return Err(ToolError::Interrupted);
    }
    let response = http.execute(request).await.map_err(|_| {
        tool_error(format!(
            "{} {} failed: transport error",
            plan.operation.method, plan.operation.path
        ))
    })?;
    runtime::read_response(plan, response)
}

fn set_tool(tools: &mut ToolTree, path: &[String], definition: Definition) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        tools.insert(head.clone(), ToolNode::Tool(definition));
        return;
    }
    if !matches!(tools.0.get(head), Some(ToolNode::Namespace(_))) {
        tools.insert(head.clone(), ToolNode::Namespace(ToolTree::new()));
    }
    if let Some(ToolNode::Namespace(child)) = tools.0.get_mut(head) {
        set_tool(child, rest, definition);
    }
}

#[cfg(test)]
mod tests {
    use super::runtime::MAX_RESPONSE_BODY_BYTES;

    #[test]
    fn response_limit_is_fifty_mebibytes() {
        assert_eq!(MAX_RESPONSE_BODY_BYTES, 52_428_800);
    }
}
