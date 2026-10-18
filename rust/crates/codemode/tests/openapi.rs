//! Purpose: port of packages/codemode/test/openapi.test.ts (OpenAPI generation and execution).
//! Scope: fixture-based (baseline fixtures read in place) with an in-process recording
//! `HttpClient`; no network. JSON cannot carry `undefined` or cycles, so the TS cases that pass
//! `[undefined]`, `{ value: undefined }` or a cyclic body are ported with the nearest JSON input
//! that exercises the same rejection path, or noted where unrepresentable.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::json
//! `-- url::Url
//! Used by:
//! `-- none, entrypoint
//! Invariants: generated tools only reach the recording client passed in `Options::http`.
//! Contract tests: self

mod common;

use common::runtime_value;
use futures::future::BoxFuture;
use opencode_codemode::openapi::{
    AuthContext, AuthResolver, Credential, Generated, HttpClient, HttpRequest, HttpResponse,
    Operation, Options, SecurityScheme, Skipped, from_spec,
};
use opencode_codemode::{
    Definition, Runtime, ToolCallContext, ToolError, ToolNode, ToolTree, input_type_script,
    output_type_script,
};
use serde_json::{Value as Json, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const BASE_URL: &str = "http://localhost:4096";

fn opencode_spec() -> Json {
    serde_json::from_str(include_str!(
        "../../../../packages/codemode/test/fixtures/opencode-v2-openapi.json"
    ))
    .expect("fixture")
}

fn happy_path_spec() -> Json {
    serde_json::from_str(include_str!(
        "../../../../packages/codemode/test/fixtures/openapi-happy-path.json"
    ))
    .expect("fixture")
}

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Json>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
    fn parsed(&self) -> url::Url {
        url::Url::parse(&self.url).expect("url")
    }
    fn param(&self, name: &str) -> Option<String> {
        self.parsed()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
    fn params(&self, name: &str) -> Vec<String> {
        self.parsed()
            .query_pairs()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .collect()
    }
}

type Respond = Arc<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>;

struct Recording {
    requests: Arc<Mutex<Vec<Recorded>>>,
    respond: Respond,
}

impl HttpClient for Recording {
    fn execute(&self, request: HttpRequest) -> BoxFuture<'static, Result<HttpResponse, String>> {
        let body = request
            .body
            .as_ref()
            .map(|bytes| serde_json::from_slice(bytes).expect("json body"));
        self.requests.lock().expect("lock").push(Recorded {
            method: request.method.clone(),
            url: request.full_url(),
            headers: request.headers.clone(),
            body,
        });
        let response = (self.respond)(&request);
        Box::pin(async move { Ok(response) })
    }
}

fn recording(
    respond: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
) -> (Arc<Mutex<Vec<Recorded>>>, Arc<dyn HttpClient>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    (
        requests.clone(),
        Arc::new(Recording {
            requests,
            respond: Arc::new(respond),
        }),
    )
}

fn json_response(value: Json, status: u16) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(&value).expect("json"),
    }
}

fn text(body: &str, content_type: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), content_type.into())],
        body: body.as_bytes().to_vec(),
    }
}

fn tool_at(tools: &ToolTree, path: &str) -> Option<Definition> {
    match tools.get(&path.split('.').collect::<Vec<_>>()) {
        Some(ToolNode::Tool(definition)) => Some(definition.clone()),
        _ => None,
    }
}

fn exists(tools: &ToolTree, path: &str) -> bool {
    tools.get(&path.split('.').collect::<Vec<_>>()).is_some()
}

async fn call(definition: &Definition, input: Json) -> Result<Json, ToolError> {
    definition
        .run
        .run(
            input,
            ToolCallContext {
                cancel: CancellationToken::new(),
            },
        )
        .await
}

fn single_operation(extra: Json, method: &str) -> Json {
    let mut operation =
        json!({ "operationId": "test", "responses": { "200": { "description": "Success" } } });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            operation.as_object_mut().expect("object").remove(&key);
        } else {
            operation[key] = value;
        }
    }
    json!({ "openapi": "3.1.0", "paths": { "/test": { method: operation } } })
}

fn with(mut spec: Json, extra: Json) -> Json {
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        spec[key] = value;
    }
    spec
}

fn generate(spec: Json, base_url: Option<&str>) -> Generated {
    from_spec(&Options {
        spec,
        base_url: base_url.map(str::to_string),
        ..Options::default()
    })
}

fn auth(
    resolve: impl Fn(AuthContext) -> Option<Credential> + Send + Sync + 'static,
) -> Arc<dyn AuthResolver> {
    let resolve = Arc::new(resolve);
    Arc::new(
        move |context: AuthContext| -> BoxFuture<'static, Result<Option<Credential>, ToolError>> {
            let credential = resolve(context);
            Box::pin(async move { Ok(credential) })
        },
    )
}

fn runtime(tools: ToolTree) -> Runtime {
    Runtime::new(opencode_codemode::Options {
        tools,
        ..opencode_codemode::Options::default()
    })
    .expect("runtime")
}

#[tokio::test]
async fn covers_a_representative_api_end_to_end() {
    let resolutions = Arc::new(Mutex::new(Vec::new()));
    let sink = resolutions.clone();
    let (requests, http) = recording(|request| {
        if request.method == "POST" {
            return HttpResponse {
                status: 201,
                headers: vec![("content-type".into(), "application/vnd.example+json".into())],
                body:
                    br#"{"id":"user-2","name":"Grace","email":"grace@example.test","role":"admin"}"#
                        .to_vec(),
            };
        }
        if request.method == "DELETE" {
            return HttpResponse {
                status: 204,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        if request.url.ends_with("/search") {
            return text("2 matches", "text/plain");
        }
        json_response(
            json!({ "id": "user-1", "name": "Ada", "email": "ada@example.test", "role": "member" }),
            200,
        )
    });
    let api = from_spec(&Options {
        spec: happy_path_spec(),
        base_url: Some(BASE_URL.into()),
        auth: Some(auth(move |context| {
            sink.lock().expect("lock").push(context.name.clone());
            Some(if context.name == "BearerAuth" {
                Credential::Bearer {
                    token: "bearer-secret".into(),
                }
            } else {
                Credential::ApiKey {
                    value: "api-secret".into(),
                }
            })
        })),
        http: Some(http),
        ..Options::default()
    });
    assert!(api.skipped.is_empty());
    let (get, create, search, remove) = (
        tool_at(&api.tools, "users.get").expect("get"),
        tool_at(&api.tools, "users.create").expect("create"),
        tool_at(&api.tools, "search.run").expect("search"),
        tool_at(&api.tools, "users.remove").expect("remove"),
    );
    assert_eq!(
        input_type_script(&get, false),
        r#"{ userId: string; include?: Array<string>; verbose?: boolean; "X-Trace-ID"?: string }"#
    );
    assert_eq!(
        input_type_script(&create, false),
        r#"{ name: string; email: string; role?: "admin" | "member" }"#
    );
    assert_eq!(
        input_type_script(&search, false),
        "{ filter?: { query: string; page?: number }; tags?: Array<string> }"
    );
    assert_eq!(input_type_script(&remove, false), "{ userId: string }");
    assert!(output_type_script(&get, false).contains("id: string"));
    assert!(output_type_script(&create, false).contains(r#"role?: "admin" | "member""#));
    assert_eq!(output_type_script(&search, false), "string");
    assert_eq!(output_type_script(&remove, false), "null");

    let result = runtime_value(
        &runtime(ToolTree::new().namespace("api", api.tools.clone())),
        r#"
          const user = await tools.api.users.get({ userId: "user-1", include: ["profile", "permissions"], verbose: true, "X-Trace-ID": "trace-1" })
          const created = await tools.api.users.create({ name: "Grace", email: "grace@example.test", role: "admin" })
          const summary = await tools.api.search.run({ filter: { query: "effect", page: 2 }, tags: ["typescript", "runtime"] })
          const removed = await tools.api.users.remove({ userId: "user-1" })
          return { user, created, summary, removed }
        "#,
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    let value = result.value.expect("value");
    assert_eq!(
        (value["user"]["id"].clone(), value["user"]["name"].clone()),
        (json!("user-1"), json!("Ada"))
    );
    assert_eq!(
        (
            value["created"]["id"].clone(),
            value["created"]["name"].clone()
        ),
        (json!("user-2"), json!("Grace"))
    );
    assert_eq!(
        (value["summary"].clone(), value["removed"].clone()),
        (json!("2 matches"), Json::Null)
    );
    assert_eq!(
        *resolutions.lock().expect("lock"),
        vec!["BearerAuth", "ApiKey", "BearerAuth"]
    );
    let requests = requests.lock().expect("lock").clone();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].parsed().path(), "/users/user-1");
    assert_eq!(
        requests[0].param("include").as_deref(),
        Some("profile,permissions")
    );
    assert_eq!(requests[0].param("verbose").as_deref(), Some("true"));
    assert_eq!(requests[0].header("x-trace-id").as_deref(), Some("trace-1"));
    assert_eq!(
        requests[0].header("authorization").as_deref(),
        Some("Bearer bearer-secret")
    );
    assert_eq!(requests[1].param("api_key").as_deref(), Some("api-secret"));
    assert_eq!(
        requests[1].body,
        Some(json!({ "name": "Grace", "email": "grace@example.test", "role": "admin" }))
    );
    assert_eq!(
        requests[2].param("filter[query]").as_deref(),
        Some("effect")
    );
    assert_eq!(requests[2].param("filter[page]").as_deref(), Some("2"));
    assert_eq!(requests[2].params("tags"), vec!["typescript", "runtime"]);
    assert_eq!(requests[2].header("authorization"), None);
    assert_eq!(requests[3].parsed().path(), "/users/user-1");
    assert_eq!(
        requests[3].header("authorization").as_deref(),
        Some("Bearer bearer-secret")
    );
}

#[test]
fn converts_representative_opencode_operations() {
    let result = generate(opencode_spec(), Some(BASE_URL));
    assert_eq!(result.skipped.len(), 5);
    assert!(result.skipped.contains(&Skipped {
        method: "GET".into(),
        path: "/api/pty/{ptyID}/connect".into(),
        reason: "WebSocket operations are not supported".into(),
    }));
    assert_eq!(
        result
            .skipped
            .iter()
            .filter(|item| item.reason == "SSE operations are not supported")
            .count(),
        3
    );
    assert!(result.skipped.contains(&Skipped {
        method: "GET".into(),
        path: "/api/fs/read/*".into(),
        reason: "binary responses are not supported".into(),
    }));
    for path in ["v2.health.get", "v2.session.get", "v2.session.create"] {
        assert!(exists(&result.tools, path), "{path}");
    }
    let session_get = tool_at(&result.tools, "v2.session.get").expect("session.get");
    assert_eq!(
        input_type_script(&session_get, false),
        "{ sessionID: string }"
    );
    assert!(output_type_script(&session_get, false).contains("id: string"));
    assert!(output_type_script(&session_get, false).contains("additions: number"));
    let switch_agent = tool_at(&result.tools, "v2.session.switchAgent").expect("switchAgent");
    assert_eq!(
        input_type_script(&switch_agent, false),
        "{ sessionID: string; agent: string }"
    );
    let context_entry =
        tool_at(&result.tools, "v2.session.contextEntry.put").expect("contextEntry.put");
    assert_eq!(
        input_type_script(&context_entry, false),
        "{ sessionID: string; key: string; value: unknown }"
    );
    for missing in [
        "v2_session_context_entry_put_2",
        "v2.pty.connect",
        "v2.session.log",
        "v2.event.subscribe",
        "v2.event.changes",
        "v2.fs.read",
    ] {
        assert!(!exists(&result.tools, missing), "{missing}");
    }
    assert!(exists(&result.tools, "v2.pty.connectToken"));
}

#[test]
fn preserves_operation_path_sanitization_and_collisions() {
    let response = json!({ "responses": { "200": { "description": "Success" } } });
    let op = |id: &str| with(response.clone(), json!({ "operationId": id }));
    let result = generate(
        json!({ "openapi": "3.1.0", "paths": {
            "/first": { "get": op("group.item") },
            "/second": { "get": op("group.item") },
            "/third": { "get": op("group..other") },
        } }),
        Some(BASE_URL),
    );
    for path in ["group.item", "group_item_2", "group.operation.other"] {
        assert!(tool_at(&result.tools, path).is_some(), "{path}");
    }
}

#[test]
fn synthesizes_flat_operation_ids() {
    let response = json!({ "responses": { "200": { "description": "Success" } } });
    let tools = generate(
        json!({ "openapi": "3.1.0", "paths": {
            "/users": { "get": response, "post": response },
            "/users/{id}": { "get": response, "patch": response, "delete": response },
            "/organizations/{organizationId}/users/{id}": { "get": response },
        } }),
        Some(BASE_URL),
    )
    .tools;
    for path in [
        "getUsers",
        "postUsers",
        "getUsersById",
        "patchUsersById",
        "deleteUsersById",
        "getOrganizationsByOrganizationidUsersById",
    ] {
        assert!(tool_at(&tools, path).is_some(), "{path}");
    }
}

#[test]
fn operation_parameters_override_path_parameters() {
    let tools = generate(
        json!({ "openapi": "3.1.0", "paths": { "/test": {
            "parameters": [{ "name": "limit", "in": "query", "schema": { "type": "string" } }],
            "get": {
                "operationId": "test",
                "parameters": [{ "name": "limit", "in": "query", "required": true, "schema": { "type": "number" } }],
                "responses": { "200": { "description": "Success" } },
            },
        } } }),
        Some(BASE_URL),
    )
    .tools;
    assert_eq!(
        input_type_script(&tool_at(&tools, "test").expect("test"), false),
        "{ limit: number }"
    );
}

#[test]
fn normalizes_openapi_3_0_schemas() {
    let tools = generate(
        json!({ "openapi": "3.0.3", "paths": { "/search": { "get": {
            "operationId": "search",
            "parameters": [{ "in": "query", "name": "value", "schema": { "type": "string", "nullable": true, "minLength": 2 } }],
            "responses": { "200": { "description": "Success" } },
        } } } }),
        Some(BASE_URL),
    )
    .tools;
    let search = tool_at(&tools, "search").expect("search");
    assert_eq!(
        input_type_script(&search, false),
        "{ value?: string | null }"
    );
    let opencode_codemode::SchemaType::Json(input) = &search.input else {
        panic!("json schema input")
    };
    assert_eq!(input["properties"]["value"]["minLength"], json!(2));
}

#[test]
fn preserves_schema_local_definitions_with_components() {
    let tools = generate(
        json!({
            "openapi": "3.1.0",
            "paths": { "/test": { "get": { "operationId": "test", "responses": { "200": {
                "description": "Success",
                "content": { "application/json": { "schema": { "$ref": "#/$defs/Local", "$defs": { "Local": { "type": "string" } } } } },
            } } } } },
            "components": { "schemas": { "Global": { "type": "number" } } },
        }),
        Some(BASE_URL),
    )
    .tools;
    let Some(opencode_codemode::SchemaType::Json(output)) =
        tool_at(&tools, "test").expect("test").output
    else {
        panic!("output")
    };
    assert_eq!(output["$defs"]["Local"], json!({ "type": "string" }));
    assert_eq!(output["$defs"]["Global"], json!({ "type": "number" }));
}

#[test]
fn documents_that_the_opencode_fixture_is_unauthenticated() {
    let spec = opencode_spec();
    assert_eq!(spec["security"], json!([]));
    assert_eq!(
        spec["components"]["securitySchemes"]
            .as_object()
            .map(|map| map.len())
            .unwrap_or(0),
        0
    );
    let health = tool_at(&generate(spec, Some(BASE_URL)).tools, "v2.health.get").expect("health");
    let opencode_codemode::SchemaType::Json(input) = &health.input else {
        panic!("json input")
    };
    assert_eq!(input["type"], json!("object"));
    assert_eq!(input["properties"], json!({}));
}

#[tokio::test]
async fn exposes_real_opencode_operations_through_discovery() {
    let (_, http) = recording(|_| json_response(json!({}), 200));
    let tools = from_spec(&Options {
        spec: opencode_spec(),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let result = runtime_value(
        &runtime(ToolTree::new().namespace("opencode", tools)),
        r#"return await tools.$codemode.search({ query: "global health", namespace: "opencode", limit: 1 })"#,
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    let value = result.value.expect("value");
    assert_eq!(
        value["items"][0]["path"],
        json!("tools.opencode.v2.health.get")
    );
    assert_eq!(
        value["items"][0]["description"],
        json!("Check whether the API server is ready to accept requests.")
    );
    assert!(value.to_string().contains("healthy: true"));
}

#[tokio::test]
async fn invokes_real_opencode_path_parameters_and_bodies() {
    let (requests, http) = recording(|request| {
        json_response(
            if request.method == "GET" {
                json!({ "id": "ses_123" })
            } else {
                json!({ "id": "ses_456" })
            },
            200,
        )
    });
    let tools = from_spec(&Options {
        spec: opencode_spec(),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let result = runtime_value(
        &runtime(ToolTree::new().namespace("opencode", tools)),
        r#"
          const existing = await tools.opencode.v2.session.get({ sessionID: "ses_123" })
          const created = await tools.opencode.v2.session.create({ id: "ses_456" })
          return { existing, created }
        "#,
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    let requests = requests.lock().expect("lock").clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        (requests[0].method.as_str(), requests[0].body.clone()),
        ("GET", None)
    );
    assert_eq!(requests[0].parsed().path(), "/api/session/ses_123");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].url, "http://localhost:4096/api/session");
    assert_eq!(requests[1].body, Some(json!({ "id": "ses_456" })));
}

#[tokio::test]
async fn serializes_deep_object_query_parameters() {
    let (requests, http) = recording(|_| json_response(json!({ "directory": "/tmp" }), 200));
    let tools = from_spec(&Options {
        spec: opencode_spec(),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let location = tool_at(&tools, "v2.location.get").expect("location");
    call(
        &location,
        json!({ "location": { "directory": "/tmp", "workspace": "workspace-1" } }),
    )
    .await
    .expect("call");
    let request = requests.lock().expect("lock")[0].clone();
    assert_eq!(
        request.param("location[directory]").as_deref(),
        Some("/tmp")
    );
    assert_eq!(
        request.param("location[workspace]").as_deref(),
        Some("workspace-1")
    );
}

#[tokio::test]
async fn serializes_supported_simple_and_form_parameter_shapes() {
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let spec = json!({ "openapi": "3.1.0", "paths": { "/items/{keys}": { "get": {
        "operationId": "items",
        "parameters": [
            { "name": "keys", "in": "path", "required": true, "schema": { "type": "array", "items": { "type": "string" } } },
            { "name": "tags", "in": "query", "style": "form", "explode": false, "schema": { "type": "array" } },
            { "name": "filter", "in": "query", "style": "form", "explode": true, "schema": { "type": "object" } },
            { "name": "nullable", "in": "query", "required": true, "schema": { "type": ["string", "null"] } },
            { "name": "constructor", "in": "query", "schema": { "type": "string" } },
            { "name": "meta", "in": "header", "style": "simple", "explode": true, "schema": { "type": "object" } },
        ],
        "responses": { "200": { "description": "Success" } },
    } } } });
    let tools = from_spec(&Options {
        spec,
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let items = tool_at(&tools, "items").expect("items");
    call(
        &items,
        json!({
            "keys": ["a!", "b*"],
            "tags": ["x", "y"],
            "filter": { "state": "open", "page": 2 },
            "nullable": null,
            "constructor_2": "safe",
            "meta": { "a": "b", "c": "d" },
        }),
    )
    .await
    .expect("call");
    let request = requests.lock().expect("lock")[0].clone();
    assert_eq!(request.parsed().path(), "/items/a%21,b%2A");
    assert_eq!(request.param("tags").as_deref(), Some("x,y"));
    assert_eq!(request.param("state").as_deref(), Some("open"));
    assert_eq!(request.param("page").as_deref(), Some("2"));
    assert_eq!(request.param("nullable").as_deref(), Some("null"));
    assert_eq!(request.param("constructor").as_deref(), Some("safe"));
    assert_eq!(request.header("meta").as_deref(), Some("a=b,c=d"));
    // TS passes `[undefined]`; JSON has no undefined, so a nested value exercises the same rejection.
    let error = call(&items, json!({ "keys": [{ "nested": true }] }))
        .await
        .expect_err("nested");
    assert!(error.to_string().contains("unsupported nested value"));
}

#[test]
fn skips_unsupported_parameter_encodings_and_malformed_security() {
    let operation = |id: &str, parameters: Json| json!({ "operationId": id, "parameters": parameters, "responses": { "200": { "description": "Success" } } });
    let result = generate(
        json!({
            "openapi": "3.1.0",
            "security": [{ "bearer": [] }],
            "paths": {
                "/cookie": { "get": operation("cookie", json!([{ "name": "session", "in": "cookie", "schema": { "type": "string" } }])) },
                "/reserved": { "get": operation("reserved", json!([{ "name": "query", "in": "query", "allowReserved": true, "schema": { "type": "string" } }])) },
                "/invalid-style": { "get": operation("invalidStyle", json!([{ "name": "query", "in": "query", "style": 42, "schema": { "type": "string" } }])) },
                "/security": { "get": { "operationId": "security", "security": null, "responses": { "200": { "description": "Success" } } } },
            },
        }),
        Some(BASE_URL),
    );
    assert!(result.tools.0.is_empty());
    assert_eq!(
        result
            .skipped
            .iter()
            .map(|item| item.reason.as_str())
            .collect::<Vec<_>>(),
        vec![
            "cookie parameter 'session' is not supported",
            "parameter 'query' uses unsupported allowReserved encoding",
            "parameter 'query' has an invalid style",
            "security declaration is not an array",
        ]
    );
}

#[test]
fn fails_closed_on_prototype_named_missing_schemes() {
    let spec: Json = serde_json::from_str(
        r#"{ "openapi": "3.1.0", "paths": { "/test": { "get": { "operationId": "test", "security": [{"__proto__":[]}], "responses": { "200": { "description": "Success" } } } } } }"#,
    )
    .expect("spec");
    let result = generate(spec, Some(BASE_URL));
    assert!(result.tools.0.is_empty());
    assert_eq!(
        result.skipped[0].reason,
        "security requirement references missing or malformed scheme: __proto__"
    );
}

#[tokio::test]
async fn resolves_bearer_auth_without_exposing_it_as_input() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let sink = contexts.clone();
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let spec = with(
        single_operation(json!({ "operationId": null }), "get"),
        json!({ "security": [{ "bearer": [] }], "components": { "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer" } } } }),
    );
    let tools = from_spec(&Options {
        spec,
        base_url: Some(BASE_URL.into()),
        auth: Some(auth(move |context| {
            sink.lock().expect("lock").push(context);
            Some(Credential::Bearer {
                token: "secret".into(),
            })
        })),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let tool = tool_at(&tools, "getTest").expect("getTest");
    call(&tool, json!({})).await.expect("call");
    assert_eq!(input_type_script(&tool, false), "{}");
    assert_eq!(
        requests.lock().expect("lock")[0]
            .header("authorization")
            .as_deref(),
        Some("Bearer secret")
    );
    assert_eq!(
        *contexts.lock().expect("lock"),
        vec![AuthContext {
            name: "bearer".into(),
            definition: SecurityScheme::Http {
                scheme: "bearer".into()
            },
            scopes: Vec::new(),
            operation: Operation {
                operation_id: None,
                method: "GET".into(),
                path: "/test".into(),
                summary: None,
                description: None
            },
        }]
    );
}

#[tokio::test]
async fn applies_auth_carriers_without_prototype_or_collision_loss() {
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let authenticated = |security: Json, schemes: Json| {
        from_spec(&Options {
            spec: with(
                single_operation(json!({}), "get"),
                json!({ "security": security, "components": { "securitySchemes": schemes } }),
            ),
            base_url: Some(BASE_URL.into()),
            auth: Some(auth(|_| {
                Some(Credential::ApiKey {
                    value: "secret".into(),
                })
            })),
            http: Some(http.clone()),
            ..Options::default()
        })
    };
    let prototype = tool_at(
        &authenticated(
            json!([{ "key": [] }]),
            json!({ "key": { "type": "apiKey", "in": "query", "name": "__proto__" } }),
        )
        .tools,
        "test",
    )
    .expect("prototype");
    call(&prototype, json!({})).await.expect("call");
    assert_eq!(
        requests.lock().expect("lock")[0]
            .param("__proto__")
            .as_deref(),
        Some("secret")
    );
    let duplicate = tool_at(
        &authenticated(
            json!([{ "first": [], "second": [] }]),
            json!({ "first": { "type": "apiKey", "in": "header", "name": "x-key" }, "second": { "type": "apiKey", "in": "header", "name": "x-key" } }),
        )
        .tools,
        "test",
    )
    .expect("duplicate");
    assert!(
        call(&duplicate, json!({}))
            .await
            .expect_err("duplicate")
            .to_string()
            .contains("multiple credentials")
    );
    let cookie = authenticated(
        json!([{ "key": [] }]),
        json!({ "key": { "type": "apiKey", "in": "cookie", "name": "session" } }),
    );
    assert!(cookie.tools.0.is_empty());
    assert_eq!(
        cookie.skipped[0].reason,
        "cookie authentication 'key' is not supported"
    );
    let alternative = from_spec(&Options {
        spec: with(
            single_operation(json!({}), "get"),
            json!({
                "security": [{ "cookie": [] }, { "bearer": [] }],
                "components": { "securitySchemes": {
                    "cookie": { "type": "apiKey", "in": "cookie", "name": "session" },
                    "bearer": { "type": "http", "scheme": "bearer" },
                } },
            }),
        ),
        base_url: Some(BASE_URL.into()),
        auth: Some(auth(|context| {
            (context.name == "bearer").then(|| Credential::Bearer {
                token: "secret".into(),
            })
        })),
        http: Some(http.clone()),
        ..Options::default()
    });
    call(
        &tool_at(&alternative.tools, "test").expect("alternative"),
        json!({}),
    )
    .await
    .expect("call");
    assert_eq!(
        requests
            .lock()
            .expect("lock")
            .last()
            .and_then(|request| request.header("authorization"))
            .as_deref(),
        Some("Bearer secret")
    );
}

#[tokio::test]
async fn honors_server_precedence_and_rejects_ambiguous_base_urls() {
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let spec = with(
        single_operation(
            json!({ "servers": [{ "url": "https://operation.example/v1" }] }),
            "get",
        ),
        json!({ "servers": [{ "url": "https://document.example" }] }),
    );
    let tools = from_spec(&Options {
        spec: spec.clone(),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    call(&tool_at(&tools, "test").expect("test"), json!({}))
        .await
        .expect("call");
    assert_eq!(
        requests.lock().expect("lock")[0].url,
        "https://operation.example/v1/test"
    );
    let invalid = generate(spec.clone(), Some("https://example.com/api?tenant=one"));
    assert!(invalid.tools.0.is_empty());
    assert!(
        invalid.skipped[0]
            .reason
            .contains("unsupported query string or fragment")
    );
    let malformed = generate(spec, Some("https:/example.com"));
    assert!(malformed.tools.0.is_empty());
    assert!(
        malformed.skipped[0]
            .reason
            .contains("not an absolute HTTP(S) URL"),
        "{}",
        malformed.skipped[0].reason
    );
}

#[test]
fn resolves_chained_response_refs_before_transport_detection() {
    let result = generate(
        with(
            single_operation(
                json!({ "responses": { "200": { "$ref": "#/components/responses/First" } } }),
                "get",
            ),
            json!({ "components": { "responses": {
                "First": { "$ref": "#/components/responses/Stream" },
                "Stream": { "content": { "text/event-stream": { "schema": { "type": "string" } } } },
            } } }),
        ),
        Some(BASE_URL),
    );
    assert!(result.tools.0.is_empty());
    assert_eq!(result.skipped[0].reason, "SSE operations are not supported");
}

#[test]
fn resolves_response_schemas_before_binary_detection() {
    let result = generate(
        with(
            single_operation(
                json!({ "responses": { "200": { "content": { "text/plain": { "schema": { "$ref": "#/components/schemas/File" } } } } } }),
                "get",
            ),
            json!({ "components": { "schemas": { "File": { "type": "string", "format": "binary" } } } }),
        ),
        Some(BASE_URL),
    );
    assert!(result.tools.0.is_empty());
    assert_eq!(
        result.skipped[0].reason,
        "binary responses are not supported"
    );
}

#[tokio::test]
async fn validates_composite_parameters_before_resolving_auth() {
    let resolutions = Arc::new(Mutex::new(Vec::new()));
    let sink = resolutions.clone();
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let tools = from_spec(&Options {
        spec: with(
            single_operation(json!({ "parameters": [{ "name": "filter", "in": "query", "style": "form", "explode": true, "schema": { "type": "object" } }] }), "get"),
            json!({ "security": [{ "bearer": [] }], "components": { "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer" } } } }),
        ),
        base_url: Some(BASE_URL.into()),
        auth: Some(auth(move |context| {
            sink.lock().expect("lock").push(context.name);
            Some(Credential::Bearer { token: "secret".into() })
        })),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    // TS passes `{ value: undefined }`; JSON has no undefined, so a nested object exercises the same rejection.
    let error = call(
        &tool_at(&tools, "test").expect("test"),
        json!({ "filter": { "value": {} } }),
    )
    .await
    .expect_err("nested");
    assert!(error.to_string().contains("unsupported nested value"));
    assert!(resolutions.lock().expect("lock").is_empty());
    assert!(requests.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn preserves_json_media_types() {
    let (requests, http) = recording(|_| json_response(json!({ "ok": true }), 200));
    let tools = from_spec(&Options {
        spec: single_operation(
            json!({ "requestBody": { "required": true, "content": { "application/merge-patch+json": { "schema": { "type": "object" } } } } }),
            "post",
        ),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    call(
        &tool_at(&tools, "test").expect("test"),
        json!({ "body": { "name": "updated" } }),
    )
    .await
    .expect("call");
    assert_eq!(
        requests.lock().expect("lock")[0]
            .header("content-type")
            .as_deref(),
        Some("application/merge-patch+json")
    );
    // The TS case also rejects a cyclic body ("Invalid JSON body"); JSON values cannot be cyclic,
    // and CodeMode's boundary rejects cyclic program values before any tool runs.
}

#[tokio::test]
async fn rejects_oversized_and_malformed_json_responses() {
    let tool = |http: Arc<dyn HttpClient>| {
        tool_at(
            &from_spec(&Options {
                spec: single_operation(json!({}), "get"),
                base_url: Some(BASE_URL.into()),
                http: Some(http),
                ..Options::default()
            })
            .tools,
            "test",
        )
        .expect("test")
    };
    let (_, oversized) = recording(|_| HttpResponse {
        status: 200,
        headers: vec![("content-length".into(), (50 * 1024 * 1024 + 1).to_string())],
        body: Vec::new(),
    });
    let (_, malformed) = recording(|_| text("{", "application/json"));
    let (_, chunked) = recording(|_| HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: vec![0u8; 50 * 1024 * 1024 + 1],
    });
    assert!(
        call(&tool(oversized), json!({}))
            .await
            .expect_err("oversized")
            .to_string()
            .contains("response exceeds 50 MiB")
    );
    assert!(
        call(&tool(malformed), json!({}))
            .await
            .expect_err("malformed")
            .to_string()
            .contains("returned malformed JSON")
    );
    assert!(
        call(&tool(chunked), json!({}))
            .await
            .expect_err("chunked")
            .to_string()
            .contains("response exceeds 50 MiB")
    );
}

#[tokio::test]
async fn keeps_non_json_responses_raw_and_unions_success_outputs() {
    let (_, http) = recording(|_| text("123", "text/plain"));
    let tools = from_spec(&Options {
        spec: single_operation(
            json!({ "responses": {
                "200": { "description": "Text", "content": { "text/plain": { "schema": { "type": "string" } } } },
                "204": { "description": "Empty" },
            } }),
            "get",
        ),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let tool = tool_at(&tools, "test").expect("test");
    assert_eq!(output_type_script(&tool, false), "string | null");
    assert_eq!(call(&tool, json!({})).await.expect("call"), json!("123"));
}

#[tokio::test]
async fn fails_missing_required_parameters_before_auth_and_network() {
    let (requests, http) = recording(|_| json_response(json!({}), 200));
    let tools = from_spec(&Options {
        spec: opencode_spec(),
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let result = runtime_value(
        &runtime(ToolTree::new().namespace("opencode", tools)),
        "return await tools.opencode.v2.session.get({})",
    )
    .await;
    assert!(!result.ok);
    assert!(
        serde_json::to_string(&result)
            .expect("json")
            .contains("Missing required path parameter 'sessionID'")
    );
    assert!(requests.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn prefixes_cross_location_collisions_and_rebuilds_requests() {
    let spec = json!({
        "openapi": "3.1.0",
        "info": { "title": "collision", "version": "1.0.0" },
        "paths": {
            "/echo": { "post": {
                "operationId": "echo",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "string" } } } },
                "responses": { "204": { "description": "Echoed" } },
            } },
            "/things/{id}": { "post": {
                "operationId": "things.update",
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } },
                    { "name": "id", "in": "query", "required": true, "schema": { "type": "string" } },
                    { "name": "path_id", "in": "query", "schema": { "type": "string" } },
                    { "name": "id", "in": "header", "required": true, "schema": { "type": "string" } },
                ],
                "requestBody": { "required": true, "content": { "application/json": { "schema": {
                    "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"], "additionalProperties": false,
                } } } },
                "responses": { "204": { "description": "Updated" } },
            } },
        },
    });
    let (requests, http) = recording(|_| HttpResponse {
        status: 204,
        headers: Vec::new(),
        body: Vec::new(),
    });
    let tools = from_spec(&Options {
        spec,
        base_url: Some(BASE_URL.into()),
        http: Some(http),
        ..Options::default()
    })
    .tools;
    let update = tool_at(&tools, "things.update").expect("update");
    assert_eq!(
        input_type_script(&update, false),
        "{ path_id: string; query_id: string; path_id_2?: string; header_id: string; body_id: string }"
    );
    assert_eq!(
        input_type_script(&tool_at(&tools, "echo").expect("echo"), false),
        "{ body: string }"
    );
    let result = runtime_value(
        &runtime(tools),
        r#"
            const updated = await tools.things.update({ path_id: "path", query_id: "query", path_id_2: "literal", header_id: "header", body_id: "body" })
            const echoed = await tools.echo({ body: "hello" })
            return { updated, echoed }
        "#,
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    let requests = requests.lock().expect("lock").clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].parsed().path(), "/things/path");
    assert_eq!(requests[0].param("id").as_deref(), Some("query"));
    assert_eq!(requests[0].param("path_id").as_deref(), Some("literal"));
    assert_eq!(requests[0].header("id").as_deref(), Some("header"));
    assert_eq!(requests[0].body, Some(json!({ "id": "body" })));
    assert_eq!(requests[1].body, Some(json!("hello")));
}

#[test]
fn keeps_bodies_nested_when_flattening_would_lose_semantics() {
    let body = |schema: Json, required: bool| json!({ "required": required, "content": { "application/json": { "schema": schema } } });
    let cases = [
        (
            "optional",
            body(
                json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"], "additionalProperties": false }),
                false,
            ),
        ),
        (
            "dictionary",
            body(
                json!({ "type": "object", "additionalProperties": { "type": "string" } }),
                true,
            ),
        ),
        (
            "composed",
            body(
                json!({ "type": "object", "allOf": [{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }], "additionalProperties": false }),
                true,
            ),
        ),
        (
            "nullable",
            body(
                json!({ "type": ["object", "null"], "properties": { "name": { "type": "string" } }, "additionalProperties": false }),
                true,
            ),
        ),
    ];
    let mut paths = serde_json::Map::new();
    for (name, request_body) in &cases {
        paths.insert(
            format!("/body/{name}"),
            json!({ "post": { "operationId": format!("body.{name}"), "requestBody": request_body, "responses": { "204": { "description": "Accepted" } } } }),
        );
    }
    let tools = generate(json!({ "openapi": "3.1.0", "info": { "title": "bodies", "version": "1.0.0" }, "paths": paths }), Some(BASE_URL)).tools;
    for (name, _) in &cases {
        let tool = tool_at(&tools, &format!("body.{name}")).expect("tool");
        let opencode_codemode::SchemaType::Json(input) = &tool.input else {
            panic!("json input")
        };
        assert_eq!(
            input["properties"]
                .as_object()
                .map(|map| map.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["body".to_string()])
        );
    }
    assert_eq!(
        input_type_script(&tool_at(&tools, "body.optional").expect("optional"), false),
        "{ body?: { name: string } }"
    );
}

#[tokio::test]
async fn tools_without_an_http_capability_refuse_to_run() {
    let tools = generate(single_operation(json!({}), "get"), Some(BASE_URL)).tools;
    let error = call(&tool_at(&tools, "test").expect("test"), json!({}))
        .await
        .expect_err("no client");
    assert!(
        error
            .to_string()
            .contains("requires an HTTP client capability")
    );
}
