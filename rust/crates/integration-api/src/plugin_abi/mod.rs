//! Purpose: Rust-defined JSON-RPC ABI between the plugin host client and the out-of-process JS
//! bridge: ABI version, method table, error codes, message schema and the generated bridge script.
//! Scope: pure values and a pure string generator. No process, file or network code.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/plugin.rs
//! `-- integration-api/src/plugin_abi/bridge.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- integration-api/src/lib.rs
//! `-- integration-api/src/plugin_abi/bridge.rs
//! Invariants: framing is one JSON-RPC 2.0 object per line on stdio. The bridge dispatch table and
//! host stubs are generated from `METHODS`, so the script cannot drift from this file. Policy
//! (module shape, ids, ordering) stays in Rust: the bridge only reports shapes and runs code.
//! Contract tests: integration-api/src/plugin_abi/bridge.rs (tests module)

mod bridge;

pub use bridge::bridge_script;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plugin::ToolContext;

/// Bumped on any incompatible change to `METHODS` or the message structs below.
pub const ABI_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Sent by the Rust host, handled by the JS bridge.
    HostToBridge,
    /// Sent by the JS bridge, handled by the Rust host (callbacks).
    BridgeToHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// Has an `id` and expects a response.
    Request,
    /// No `id`, fire-and-forget.
    Notification,
}

/// One ABI method. `js` is the bridge identifier: the handler function name for host-to-bridge
/// methods, or the generated `host.<js>(params)` stub name for bridge-to-host methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub direction: Direction,
    pub kind: CallKind,
    pub js: &'static str,
}

const fn m(
    name: &'static str,
    direction: Direction,
    kind: CallKind,
    js: &'static str,
) -> MethodSpec {
    MethodSpec {
        name,
        direction,
        kind,
        js,
    }
}

use CallKind::{Notification, Request};
use Direction::{BridgeToHost, HostToBridge};

pub const INITIALIZE: &str = "initialize";
pub const MODULE_IMPORT: &str = "module.import";
pub const PLUGIN_INIT: &str = "plugin.init";
pub const HOOK_TRIGGER: &str = "hook.trigger";
pub const HOOK_EVENT: &str = "hook.event";
pub const HOOK_CONFIG: &str = "hook.config";
pub const HOOK_DISPOSE: &str = "hook.dispose";
pub const TOOL_EXECUTE: &str = "tool.execute";
pub const TOOL_ABORT: &str = "tool.abort";
pub const AUTH_METHODS: &str = "auth.methods";
pub const AUTH_AUTHORIZE: &str = "auth.authorize";
pub const AUTH_CANCEL: &str = "auth.cancel";
pub const AUTH_CALLBACK: &str = "auth.callback";
pub const SHUTDOWN: &str = "shutdown";
pub const CTX_ASK: &str = "ctx.ask";
pub const CTX_METADATA: &str = "ctx.metadata";
pub const CLIENT_CALL: &str = "client.call";
pub const WORKSPACE_REGISTER: &str = "workspace.register";
pub const LOG: &str = "bridge.log";

/// The complete method table. The bridge script is generated from this.
pub const METHODS: &[MethodSpec] = &[
    m(INITIALIZE, HostToBridge, Request, "onInitialize"),
    m(MODULE_IMPORT, HostToBridge, Request, "onModuleImport"),
    m(PLUGIN_INIT, HostToBridge, Request, "onPluginInit"),
    m(HOOK_TRIGGER, HostToBridge, Request, "onHookTrigger"),
    m(HOOK_EVENT, HostToBridge, Notification, "onHookEvent"),
    m(HOOK_CONFIG, HostToBridge, Request, "onHookConfig"),
    m(HOOK_DISPOSE, HostToBridge, Request, "onHookDispose"),
    m(TOOL_EXECUTE, HostToBridge, Request, "onToolExecute"),
    m(TOOL_ABORT, HostToBridge, Notification, "onToolAbort"),
    m(AUTH_METHODS, HostToBridge, Request, "onAuthMethods"),
    m(AUTH_AUTHORIZE, HostToBridge, Request, "onAuthAuthorize"),
    m(AUTH_CANCEL, HostToBridge, Request, "onAuthCancel"),
    m(AUTH_CALLBACK, HostToBridge, Request, "onAuthCallback"),
    m(SHUTDOWN, HostToBridge, Request, "onShutdown"),
    m(CTX_ASK, BridgeToHost, Request, "ctxAsk"),
    m(CTX_METADATA, BridgeToHost, Notification, "ctxMetadata"),
    m(CLIENT_CALL, BridgeToHost, Request, "clientCall"),
    m(
        WORKSPACE_REGISTER,
        BridgeToHost,
        Notification,
        "workspaceRegister",
    ),
    m(LOG, BridgeToHost, Notification, "log"),
];

pub fn method(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.name == name)
}

/// JSON-RPC error codes. Negative codes are JSON-RPC standard, positive ones are ABI specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Internal,
    /// Plugin code threw or rejected.
    PluginThrew,
    /// Unknown module handle, plugin handle, hook or tool.
    NotFound,
    /// Tool args failed the plugin's zod schema.
    Validation,
    /// Behavior the bridge or host does not support (typed gap).
    Unsupported,
    AbiMismatch,
    /// The call was aborted via `tool.abort`.
    Aborted,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 11] = [
        Self::ParseError,
        Self::InvalidRequest,
        Self::MethodNotFound,
        Self::InvalidParams,
        Self::Internal,
        Self::PluginThrew,
        Self::NotFound,
        Self::Validation,
        Self::Unsupported,
        Self::AbiMismatch,
        Self::Aborted,
    ];

    pub fn code(self) -> i64 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::Internal => -32603,
            Self::PluginThrew => 1000,
            Self::NotFound => 1001,
            Self::Validation => 1002,
            Self::Unsupported => 1003,
            Self::AbiMismatch => 1004,
            Self::Aborted => 1005,
        }
    }

    pub fn js_name(self) -> &'static str {
        match self {
            Self::ParseError => "ParseError",
            Self::InvalidRequest => "InvalidRequest",
            Self::MethodNotFound => "MethodNotFound",
            Self::InvalidParams => "InvalidParams",
            Self::Internal => "Internal",
            Self::PluginThrew => "PluginThrew",
            Self::NotFound => "NotFound",
            Self::Validation => "Validation",
            Self::Unsupported => "Unsupported",
            Self::AbiMismatch => "AbiMismatch",
            Self::Aborted => "Aborted",
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub abi_version: u32,
    /// Absolute path of a zod module used when a plugin's own `zod` cannot be resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zod_fallback: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub abi_version: u32,
    /// `bun` or `node`.
    pub runtime: String,
    pub runtime_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportParams {
    /// `file://` URL of the resolved entrypoint.
    pub entry: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub module: u64,
    /// `Object.entries(mod)` in export order.
    pub exports: Vec<ExportShape>,
}

/// Shape of one module export, enough for Rust to apply `readV1Plugin` / `getLegacyPlugins`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportShape {
    pub name: String,
    /// `typeof value`, except `null` and `array` are reported as such.
    #[serde(rename = "type")]
    pub kind: String,
    /// Same number for exports that are the same JS value (`===`).
    pub identity: u64,
    /// `typeof value.id` when `"id" in value` (records only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_type: Option<String>,
    /// `value.id` when it is a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `typeof value.server` when `"server" in value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// `typeof value.tui` when `"tui" in value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InitMember {
    /// The export itself is the plugin function.
    #[serde(rename = "self")]
    Itself,
    /// The export's `server` member is the plugin function.
    Server,
}

/// Serializable part of `PluginInput`. `client`, `$` and `experimental_workspace` are built by the
/// bridge (host callbacks or the interpreter's own `Bun.$`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInputWire {
    pub directory: String,
    pub worktree: String,
    pub project: Value,
    pub server_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitParams {
    pub module: u64,
    pub export: String,
    pub member: InitMember,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    pub input: PluginInputWire,
}

/// Tool args as seen by the bridge, before Rust normalizes them into JSON Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ArgsShape {
    /// All args are zod types: raw `z.toJSONSchema(z.object(args), { io: "input" })`.
    Zod { schema: Value },
    /// Not all args are zod: JSON-cloned entries (non-JSON values become null).
    Legacy { entries: Map<String, Value> },
    /// Schema generation threw.
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolShape {
    pub id: String,
    pub description: String,
    pub args: ArgsShape,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HooksShape {
    /// Function-valued hook keys that are in the trigger table, in declaration order.
    pub triggers: Vec<String>,
    pub event: bool,
    pub config: bool,
    pub dispose: bool,
    pub tools: Vec<ToolShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitResult {
    pub handle: u64,
    pub hooks: HooksShape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerParams {
    pub handle: u64,
    pub name: String,
    pub input: Value,
    pub output: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerResult {
    pub output: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventParams {
    pub handle: u64,
    pub event: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigParams {
    pub handle: u64,
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandleParams {
    pub handle: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecuteParams {
    pub handle: u64,
    pub tool: String,
    pub args: Value,
    pub context: ToolContext,
    pub call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallParams {
    pub call_id: String,
}

/// `ctx.ask` / `ctx.metadata` payloads: the plugin's argument, untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallbackParams {
    pub call_id: String,
    pub input: Value,
}

/// A `client.<path>(...args)` call made by plugin code on the bridged SDK client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCallParams {
    pub path: Vec<String>,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRegisterParams {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogParams {
    pub level: String,
    pub message: String,
}
