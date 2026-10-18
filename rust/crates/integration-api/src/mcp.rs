//! Purpose: MCP client domain contract: server config, status union, catalog entries, prompt and
//! resource values, auth status and the `McpService` trait consumed by sessions, tools and routes.
//! Scope: serializable values plus one async trait. Transports live in integration-adapters.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- integration-api/src/lib.rs
//! Invariants: JSON shapes match packages/core/src/v1/config/mcp.ts and the MCPStatus union in
//! packages/opencode/src/mcp/index.ts:83-107 (discriminator `status`, optional keys omitted).
//! Contract tests: integration-api/src/mcp.rs (tests module)

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::IntegrationError;

/// Default request timeout when config omits `timeout` (baseline catalog.ts DEFAULT_TIMEOUT).
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Pagination guard (baseline catalog.ts MAX_LIST_PAGES).
pub const MAX_LIST_PAGES: usize = 1_000;
/// Default OAuth callback port and path (baseline oauth-callback.ts).
pub const OAUTH_CALLBACK_PORT: u16 = 19_876;
pub const OAUTH_CALLBACK_PATH: &str = "/mcp/oauth/callback";

/// `McpLocalConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalConfig {
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// `McpOAuthConfig`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
}

/// `oauth: McpOAuthConfig | false`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OAuthSetting {
    Disabled(FalseLiteral),
    Config(OAuthConfig),
}

/// Serializes as JSON `false`, rejects `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalseLiteral;

impl Serialize for FalseLiteral {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for FalseLiteral {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match bool::deserialize(d)? {
            false => Ok(FalseLiteral),
            true => Err(serde::de::Error::custom("expected false")),
        }
    }
}

/// `McpRemoteConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// `ConfigMCPV1.Info`, discriminated by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerConfig {
    Local(LocalConfig),
    Remote(RemoteConfig),
}

impl ServerConfig {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Local(c) => c.enabled != Some(false),
            Self::Remote(c) => c.enabled != Some(false),
        }
    }

    pub fn timeout_ms(&self) -> Option<u64> {
        match self {
            Self::Local(c) => c.timeout,
            Self::Remote(c) => c.timeout,
        }
    }
}

/// `MCPStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    Connected,
    Disabled,
    Failed { error: String },
    NeedsAuth,
    NeedsClientRegistration { error: String },
}

/// Baseline `AuthStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Authenticated,
    Expired,
    NotAuthenticated,
}

/// Native MCP tool definition (`Tool` from the MCP spec). Unknown fields are preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A catalog entry keyed by sanitized `<client>_<tool>` name.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub client: String,
    pub def: ToolDef,
    pub timeout_ms: Option<u64>,
}

/// `ServerInstructions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInstructions {
    pub name: String,
    pub instructions: String,
    pub tools: Vec<String>,
}

/// `McpResource` (route DTO) and the catalog resource entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub client: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplate {
    pub name: String,
    pub uri_template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub client: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
    pub client: String,
}

/// Raw MCP results are passed through as JSON to keep spec fields byte-compatible.
pub type CallToolResult = Value;
pub type GetPromptResult = Value;
pub type ReadResourceResult = Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedAuth {
    pub authorization_url: String,
    pub oauth_state: String,
}

/// Bus events the MCP domain publishes (`mcp.tools.changed`, `mcp.browser.open.failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpEvent {
    #[serde(rename = "mcp.tools.changed")]
    ToolsChanged { properties: ToolsChangedProps },
    #[serde(rename = "mcp.browser.open.failed")]
    BrowserOpenFailed { properties: BrowserOpenFailedProps },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsChangedProps {
    pub server: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOpenFailedProps {
    pub mcp_name: String,
    pub url: String,
}

/// Per-Location MCP service, mirrors `MCP.Interface` in packages/opencode/src/mcp/index.ts:164-200.
#[async_trait]
pub trait McpService: Send + Sync {
    async fn status(&self) -> BTreeMap<String, Status>;
    async fn instructions(&self) -> Vec<ServerInstructions>;
    async fn tools(&self) -> BTreeMap<String, McpTool>;
    async fn prompts(&self) -> BTreeMap<String, Prompt>;
    async fn resources(&self, client: Option<&str>) -> BTreeMap<String, Resource>;
    async fn resource_templates(&self, client: Option<&str>) -> BTreeMap<String, ResourceTemplate>;
    async fn add(
        &self,
        name: &str,
        config: ServerConfig,
    ) -> Result<BTreeMap<String, Status>, IntegrationError>;
    async fn connect(&self, name: &str) -> Result<(), IntegrationError>;
    async fn disconnect(&self, name: &str) -> Result<(), IntegrationError>;
    async fn call_tool(
        &self,
        client: &str,
        tool: &str,
        arguments: Map<String, Value>,
        timeout_ms: Option<u64>,
    ) -> Result<CallToolResult, IntegrationError>;
    async fn get_prompt(
        &self,
        client: &str,
        name: &str,
        arguments: Option<BTreeMap<String, String>>,
    ) -> Result<Option<GetPromptResult>, IntegrationError>;
    async fn read_resource(
        &self,
        client: &str,
        uri: &str,
    ) -> Result<Option<ReadResourceResult>, IntegrationError>;
    async fn start_auth(&self, name: &str) -> Result<StartedAuth, IntegrationError>;
    async fn authenticate(&self, name: &str) -> Result<Status, IntegrationError>;
    async fn finish_auth(&self, name: &str, code: &str) -> Result<Status, IntegrationError>;
    async fn remove_auth(&self, name: &str) -> Result<(), IntegrationError>;
    async fn supports_oauth(&self, name: &str) -> Result<bool, IntegrationError>;
    async fn has_stored_tokens(&self, name: &str) -> bool;
    async fn auth_status(&self, name: &str) -> AuthStatus;
}

/// Baseline sanitization for catalog keys: non `[a-zA-Z0-9_-]` become `_`.
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_union_wire() {
        assert_eq!(
            serde_json::to_value(Status::Connected).unwrap(),
            json!({"status":"connected"})
        );
        assert_eq!(
            serde_json::to_value(Status::NeedsClientRegistration { error: "x".into() }).unwrap(),
            json!({"status":"needs_client_registration","error":"x"})
        );
    }

    #[test]
    fn config_union_and_oauth_false() {
        let c: ServerConfig =
            serde_json::from_value(json!({"type":"remote","url":"https://a","oauth":false}))
                .unwrap();
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"type":"remote","url":"https://a","oauth":false})
        );
        assert!(
            serde_json::from_value::<ServerConfig>(json!({"type":"remote","url":"u","oauth":true}))
                .is_err()
        );
        let l: ServerConfig =
            serde_json::from_value(json!({"type":"local","command":["x"],"enabled":false}))
                .unwrap();
        assert!(!l.enabled());
    }

    #[test]
    fn tool_def_preserves_unknown_fields() {
        let v = json!({"name":"t","inputSchema":{"type":"object"},"_meta":{"a":1}});
        let d: ToolDef = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(d).unwrap(), v);
    }
}
