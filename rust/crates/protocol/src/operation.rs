//! Purpose: operation registry types (API, group, endpoint) shared by server routing, OpenAPI
//! emission and client codegen. Does not route or encode requests.
//! Scope: pure data plus a fluent endpoint builder.
//! Uses:
//! rust/crates/
//! `-- protocol/src/ir.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- protocol/src/current/mod.rs
//! |-- protocol/src/legacy/mod.rs
//! `-- protocol/src/lib.rs
//! Invariants: `Endpoint::name` is the TS `HttpApiEndpoint` name, `operation_id` the OpenAPI
//! identifier. Path templates use `:param` (and a trailing `*` wildcard) like the baseline.
//! Group and endpoint order is registration order.
//! Contract tests: protocol/tests/registry.rs

use crate::ir::{Definition, Field, Schema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

/// Request payload media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PayloadEncoding {
    Json,
    Multipart,
    Form,
    Text,
    Bytes,
}

/// Success body encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuccessEncoding {
    Json,
    /// `text/event-stream`, schema describes each frame's `data`.
    Sse,
    /// No body (204 `HttpApiSchema.NoContent`).
    Void,
    /// `application/octet-stream`.
    Bytes,
    /// WebSocket upgrade, schema describes nothing.
    WebSocket,
}

/// Middleware applied to an endpoint, in application order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Middleware {
    /// `Authorization`, error `UnauthorizedError` (401).
    Auth,
    /// `SchemaErrorMiddleware`, error `InvalidRequestError` (400).
    SchemaError,
    /// Location resolution from bracketed query/header hints.
    Location,
    /// Session placement from persisted session data.
    SessionLocation,
    /// Legacy instance/workspace routing (directory/workspace hints, remote proxy).
    LegacyInstance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Success {
    pub status: u16,
    pub schema: Option<Schema>,
    pub encoding: SuccessEncoding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub status: u16,
    /// Named error definition (e.g. `SessionNotFoundError`) or inline schema.
    pub schema: Schema,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub name: String,
    pub operation_id: String,
    pub method: Method,
    pub path: String,
    /// Path params in template order.
    pub params: Vec<Field>,
    /// Query fields (struct fields, flattened like Effect).
    pub query: Vec<Field>,
    /// Query parameters rendered as OpenAPI `deepObject` (bracketed `location[directory]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deep_object_query: Vec<String>,
    pub headers: Vec<Field>,
    pub payload: Option<Schema>,
    pub payload_encoding: PayloadEncoding,
    pub payload_required: bool,
    pub successes: Vec<Success>,
    pub errors: Vec<ErrorResponse>,
    pub middleware: Vec<Middleware>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// Effect group identifier, e.g. `server.session`.
    pub identifier: String,
    /// OpenAPI tag title, e.g. `sessions`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub top_level: bool,
    pub legacy: bool,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Api {
    pub identifier: String,
    pub title: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub groups: Vec<Group>,
    /// Named schemas in registration order.
    pub definitions: Vec<Definition>,
}

impl Api {
    pub fn endpoints(&self) -> impl Iterator<Item = (&Group, &Endpoint)> {
        self.groups
            .iter()
            .flat_map(|g| g.endpoints.iter().map(move |e| (g, e)))
    }

    pub fn find(&self, name: &str) -> Option<&Endpoint> {
        self.endpoints().map(|(_, e)| e).find(|e| e.name == name)
    }

    pub fn definition(&self, name: &str) -> Option<&Definition> {
        self.definitions.iter().find(|d| d.name == name)
    }
}

impl Endpoint {
    pub fn new(name: &str, operation_id: &str, method: Method, path: &str) -> Self {
        Endpoint {
            name: name.to_string(),
            operation_id: operation_id.to_string(),
            method,
            path: path.to_string(),
            params: Vec::new(),
            query: Vec::new(),
            deep_object_query: Vec::new(),
            headers: Vec::new(),
            payload: None,
            payload_encoding: PayloadEncoding::Json,
            payload_required: false,
            successes: Vec::new(),
            errors: Vec::new(),
            middleware: Vec::new(),
            summary: None,
            description: None,
            deprecated: false,
        }
    }

    pub fn params(mut self, fields: Vec<Field>) -> Self {
        self.params = fields;
        self
    }

    pub fn query(mut self, fields: Vec<Field>) -> Self {
        self.query = fields;
        self
    }

    /// Mark `location` (or another struct query field) as deepObject style.
    pub fn deep(mut self, name: &str) -> Self {
        self.deep_object_query.push(name.to_string());
        self
    }

    pub fn headers(mut self, fields: Vec<Field>) -> Self {
        self.headers = fields;
        self
    }

    pub fn payload(mut self, schema: Schema) -> Self {
        self.payload = Some(schema);
        self.payload_required = true;
        self
    }

    pub fn payload_encoding(mut self, encoding: PayloadEncoding) -> Self {
        self.payload_encoding = encoding;
        self
    }

    pub fn ok(self, schema: Schema) -> Self {
        self.success(200, Some(schema), SuccessEncoding::Json)
    }

    pub fn no_content(self) -> Self {
        self.success(204, None, SuccessEncoding::Void)
    }

    pub fn sse(self, schema: Schema) -> Self {
        self.success(200, Some(schema), SuccessEncoding::Sse)
    }

    pub fn bytes(self) -> Self {
        self.success(200, None, SuccessEncoding::Bytes)
    }

    pub fn websocket(self) -> Self {
        self.success(101, None, SuccessEncoding::WebSocket)
    }

    pub fn success(
        mut self,
        status: u16,
        schema: Option<Schema>,
        encoding: SuccessEncoding,
    ) -> Self {
        self.successes.push(Success {
            status,
            schema,
            encoding,
            description: None,
        });
        self
    }

    pub fn error(mut self, status: u16, schema: Schema) -> Self {
        self.errors.push(ErrorResponse { status, schema });
        self
    }

    /// Add a named protocol error (see `crate::errors::ErrorKind`).
    pub fn err(self, kind: crate::errors::ErrorKind) -> Self {
        self.error(kind.status(), crate::ir::r(kind.tag()))
    }

    pub fn middleware(mut self, middleware: Middleware) -> Self {
        self.middleware.push(middleware);
        self
    }

    pub fn doc(mut self, summary: &str, description: &str) -> Self {
        self.summary = Some(summary.to_string());
        if !description.is_empty() {
            self.description = Some(description.to_string());
        }
        self
    }

    pub fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }
}

impl Group {
    pub fn new(identifier: &str, title: Option<&str>, description: Option<&str>) -> Self {
        Group {
            identifier: identifier.to_string(),
            title: title.map(str::to_string),
            description: description.map(str::to_string),
            top_level: false,
            legacy: false,
            endpoints: Vec::new(),
        }
    }

    pub fn endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoints.push(endpoint);
        self
    }

    /// Apply a middleware to every endpoint registered so far (Effect `Group.middleware`).
    pub fn with_middleware(mut self, middleware: Middleware) -> Self {
        for endpoint in &mut self.endpoints {
            endpoint.middleware.push(middleware);
        }
        self
    }
}
