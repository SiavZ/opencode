//! Purpose: `catalog.ts`, `models-dev.ts`, `plugin.ts`, `ide/installation/lsp/mcp/server/tui/vcs-event.ts` events.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: byte-identical JSON with the baseline module.
//! Contract tests: schema/tests/schema_domains_misc.rs

use crate::event::{Definition, EventData};
use crate::ids::{PluginID, SessionID};
use crate::values::{PositiveInt, optional};
use serde::{Deserialize, Serialize};

// ============================================================================
// Catalog
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogUpdated {}

impl EventData for CatalogUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "catalog.updated",
        durable: None,
    };
}

// ============================================================================
// Models Dev
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsDevRefreshed {}

impl EventData for ModelsDevRefreshed {
    const DEFINITION: Definition = Definition {
        r#type: "models-dev.refreshed",
        durable: None,
    };
}

// ============================================================================
// Plugin
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAdded {
    pub id: PluginID,
}

impl EventData for PluginAdded {
    const DEFINITION: Definition = Definition {
        r#type: "plugin.added",
        durable: None,
    };
}

// ============================================================================
// IDE Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeInstalled {
    pub ide: String,
}

impl EventData for IdeInstalled {
    const DEFINITION: Definition = Definition {
        r#type: "ide.installed",
        durable: None,
    };
}

// ============================================================================
// Installation Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationUpdated {
    pub version: String,
}

impl EventData for InstallationUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "installation.updated",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationUpdateAvailable {
    pub version: String,
}

impl EventData for InstallationUpdateAvailable {
    const DEFINITION: Definition = Definition {
        r#type: "installation.update-available",
        durable: None,
    };
}

// ============================================================================
// LSP Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspUpdated {}

impl EventData for LspUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "lsp.updated",
        durable: None,
    };
}

// ============================================================================
// MCP Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolsChanged {
    pub server: String,
}

impl EventData for McpToolsChanged {
    const DEFINITION: Definition = Definition {
        r#type: "mcp.tools.changed",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpBrowserOpenFailed {
    #[serde(rename = "mcpName")]
    pub mcp_name: String,
    pub url: String,
}

impl EventData for McpBrowserOpenFailed {
    const DEFINITION: Definition = Definition {
        r#type: "mcp.browser.open.failed",
        durable: None,
    };
}

// ============================================================================
// Server Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConnected {}

impl EventData for ServerConnected {
    const DEFINITION: Definition = Definition {
        r#type: "server.connected",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalDisposed {}

impl EventData for GlobalDisposed {
    const DEFINITION: Definition = Definition {
        r#type: "global.disposed",
        durable: None,
    };
}

// ============================================================================
// TUI Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiPromptAppend {
    pub text: String,
}

impl EventData for TuiPromptAppend {
    const DEFINITION: Definition = Definition {
        r#type: "tui.prompt.append",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiCommandExecute {
    pub command: String,
}

impl EventData for TuiCommandExecute {
    const DEFINITION: Definition = Definition {
        r#type: "tui.command.execute",
        durable: None,
    };
}

fn default_toast_duration() -> PositiveInt {
    PositiveInt::new(5000).unwrap()
}

fn deserialize_toast_duration<'de, D>(deserializer: D) -> Result<PositiveInt, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Err(serde::de::Error::custom("Expected a value, got null"));
    }
    let parsed: i64 = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
    PositiveInt::new(parsed)
        .ok_or_else(|| serde::de::Error::custom("Expected a value greater than 0"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastVariant {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiToastShow {
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub title: std::option::Option<String>,
    pub message: String,
    pub variant: ToastVariant,
    #[serde(
        default = "default_toast_duration",
        deserialize_with = "deserialize_toast_duration"
    )]
    pub duration: PositiveInt,
}

impl EventData for TuiToastShow {
    const DEFINITION: Definition = Definition {
        r#type: "tui.toast.show",
        durable: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiSessionSelect {
    #[serde(rename = "sessionID")]
    pub session_id: SessionID,
}

impl EventData for TuiSessionSelect {
    const DEFINITION: Definition = Definition {
        r#type: "tui.session.select",
        durable: None,
    };
}

// ============================================================================
// VCS Event
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcsBranchUpdated {
    #[serde(
        default,
        skip_serializing_if = "std::option::Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub branch: std::option::Option<String>,
}

impl EventData for VcsBranchUpdated {
    const DEFINITION: Definition = Definition {
        r#type: "vcs.branch.updated",
        durable: None,
    };
}

// ============================================================================
// Definitions Inventory
// ============================================================================

pub const DEFINITIONS: &[Definition] = &[
    CatalogUpdated::DEFINITION,
    ModelsDevRefreshed::DEFINITION,
    PluginAdded::DEFINITION,
    IdeInstalled::DEFINITION,
    InstallationUpdated::DEFINITION,
    InstallationUpdateAvailable::DEFINITION,
    LspUpdated::DEFINITION,
    McpToolsChanged::DEFINITION,
    McpBrowserOpenFailed::DEFINITION,
    ServerConnected::DEFINITION,
    GlobalDisposed::DEFINITION,
    TuiPromptAppend::DEFINITION,
    TuiCommandExecute::DEFINITION,
    TuiToastShow::DEFINITION,
    TuiSessionSelect::DEFINITION,
    VcsBranchUpdated::DEFINITION,
];
