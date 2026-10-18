//! Purpose: verify misc events (catalog, models-dev, plugin, ide, installation, lsp, mcp, server, tui, vcs) against baseline captures.
//! Scope: schema domain conformance tests.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/misc_event.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are Bun output, JS trim and optional null rejection are preserved.
//! Contract tests: cargo test -p opencode-schema --test schema_domains_misc

use opencode_schema::event::Event;
use opencode_schema::misc_event::{
    CatalogUpdated, DEFINITIONS, GlobalDisposed, IdeInstalled, InstallationUpdateAvailable,
    InstallationUpdated, LspUpdated, McpBrowserOpenFailed, McpToolsChanged, ModelsDevRefreshed,
    PluginAdded, ServerConnected, TuiCommandExecute, TuiPromptAppend, TuiSessionSelect,
    TuiToastShow, VcsBranchUpdated,
};
use serde::{Serialize, de::DeserializeOwned};

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), fixture);
}

// fixture: captured from bun at baseline, 2026-10-15, /tmp/p01home/misc.ts.
const CATALOG_UPDATED: &str = r#"{"id":"evt_1","type":"catalog.updated","data":{}}"#;
const MODELS_DEV_REFRESHED: &str = r#"{"id":"evt_2","type":"models-dev.refreshed","data":{}}"#;
const PLUGIN_ADDED: &str = r#"{"id":"evt_3","type":"plugin.added","data":{"id":"plug_1"}}"#;
const IDE_INSTALLED: &str = r#"{"id":"evt_4","type":"ide.installed","data":{"ide":"vscode"}}"#;
const INSTALLATION_UPDATED: &str =
    r#"{"id":"evt_5","type":"installation.updated","data":{"version":"1.0.0"}}"#;
const INSTALLATION_UPDATE_AVAILABLE: &str =
    r#"{"id":"evt_6","type":"installation.update-available","data":{"version":"1.1.0"}}"#;
const LSP_UPDATED: &str = r#"{"id":"evt_7","type":"lsp.updated","data":{}}"#;
const MCP_TOOLS_CHANGED: &str =
    r#"{"id":"evt_8","type":"mcp.tools.changed","data":{"server":"github"}}"#;
const MCP_BROWSER_OPEN_FAILED: &str = r#"{"id":"evt_9","type":"mcp.browser.open.failed","data":{"mcpName":"github","url":"https://example.com"}}"#;
const SERVER_CONNECTED: &str = r#"{"id":"evt_10","type":"server.connected","data":{}}"#;
const GLOBAL_DISPOSED: &str = r#"{"id":"evt_11","type":"global.disposed","data":{}}"#;
const TUI_PROMPT_APPEND: &str =
    r#"{"id":"evt_12","type":"tui.prompt.append","data":{"text":"hello"}}"#;
const TUI_COMMAND_EXECUTE: &str =
    r#"{"id":"evt_13","type":"tui.command.execute","data":{"command":"session.list"}}"#;
const TUI_TOAST_SHOW: &str = r#"{"id":"evt_14","type":"tui.toast.show","data":{"message":"done","variant":"success","duration":5000}}"#;
const TUI_TOAST_SHOW_WITH_TITLE: &str = r#"{"id":"evt_15","type":"tui.toast.show","data":{"title":"Info","message":"done","variant":"info","duration":3000}}"#;
const TUI_SESSION_SELECT: &str =
    r#"{"id":"evt_16","type":"tui.session.select","data":{"sessionID":"ses_1"}}"#;
const VCS_BRANCH_UPDATED: &str =
    r#"{"id":"evt_17","type":"vcs.branch.updated","data":{"branch":"main"}}"#;
const VCS_BRANCH_UPDATED_NO_BRANCH: &str =
    r#"{"id":"evt_18","type":"vcs.branch.updated","data":{}}"#;

#[test]
fn fixture_captured_from_bun_at_baseline() {
    roundtrip::<Event<CatalogUpdated>>(CATALOG_UPDATED);
    roundtrip::<Event<ModelsDevRefreshed>>(MODELS_DEV_REFRESHED);
    roundtrip::<Event<PluginAdded>>(PLUGIN_ADDED);
    roundtrip::<Event<IdeInstalled>>(IDE_INSTALLED);
    roundtrip::<Event<InstallationUpdated>>(INSTALLATION_UPDATED);
    roundtrip::<Event<InstallationUpdateAvailable>>(INSTALLATION_UPDATE_AVAILABLE);
    roundtrip::<Event<LspUpdated>>(LSP_UPDATED);
    roundtrip::<Event<McpToolsChanged>>(MCP_TOOLS_CHANGED);
    roundtrip::<Event<McpBrowserOpenFailed>>(MCP_BROWSER_OPEN_FAILED);
    roundtrip::<Event<ServerConnected>>(SERVER_CONNECTED);
    roundtrip::<Event<GlobalDisposed>>(GLOBAL_DISPOSED);
    roundtrip::<Event<TuiPromptAppend>>(TUI_PROMPT_APPEND);
    roundtrip::<Event<TuiCommandExecute>>(TUI_COMMAND_EXECUTE);
    roundtrip::<Event<TuiToastShow>>(TUI_TOAST_SHOW);
    roundtrip::<Event<TuiToastShow>>(TUI_TOAST_SHOW_WITH_TITLE);
    roundtrip::<Event<TuiSessionSelect>>(TUI_SESSION_SELECT);
    roundtrip::<Event<VcsBranchUpdated>>(VCS_BRANCH_UPDATED);
    roundtrip::<Event<VcsBranchUpdated>>(VCS_BRANCH_UPDATED_NO_BRANCH);
}

#[test]
fn tui_toast_duration_defaults_to_5000_and_rejects_null_or_invalid() {
    // Missing duration defaults to 5000
    let input =
        r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"info"}}"#;
    let decoded: Event<TuiToastShow> = serde_json::from_str(input).unwrap();
    assert_eq!(decoded.data.duration.get(), 5000);
    // When serialized, the default 5000 should be present (since it's not skipped)
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"info","duration":5000}}"#
    );

    // Null duration is rejected
    let null_input = r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"info","duration":null}}"#;
    assert!(serde_json::from_str::<Event<TuiToastShow>>(null_input).is_err());

    // Invalid (<= 0) duration is rejected
    let zero_input = r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"info","duration":0}}"#;
    assert!(serde_json::from_str::<Event<TuiToastShow>>(zero_input).is_err());

    let negative_input = r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"info","duration":-1}}"#;
    assert!(serde_json::from_str::<Event<TuiToastShow>>(negative_input).is_err());
}

#[test]
fn optional_fields_omit_missing_and_reject_null() {
    // VCS branch
    let null_branch = r#"{"id":"evt_x","type":"vcs.branch.updated","data":{"branch":null}}"#;
    assert!(serde_json::from_str::<Event<VcsBranchUpdated>>(null_branch).is_err());

    // TUI toast title
    let null_title = r#"{"id":"evt_x","type":"tui.toast.show","data":{"title":null,"message":"test","variant":"info","duration":1000}}"#;
    assert!(serde_json::from_str::<Event<TuiToastShow>>(null_title).is_err());
}

#[test]
fn inventory_order_matches_baseline() {
    let types: Vec<&str> = DEFINITIONS.iter().map(|d| d.r#type).collect();
    assert_eq!(
        types,
        vec![
            "catalog.updated",
            "models-dev.refreshed",
            "plugin.added",
            "ide.installed",
            "installation.updated",
            "installation.update-available",
            "lsp.updated",
            "mcp.tools.changed",
            "mcp.browser.open.failed",
            "server.connected",
            "global.disposed",
            "tui.prompt.append",
            "tui.command.execute",
            "tui.toast.show",
            "tui.session.select",
            "vcs.branch.updated",
        ]
    );
}

#[test]
fn invalid_ids_and_scalars_are_rejected() {
    // SessionID requires "ses" prefix
    let bad_session = r#"{"id":"evt_x","type":"tui.session.select","data":{"sessionID":"bad_id"}}"#;
    assert!(serde_json::from_str::<Event<TuiSessionSelect>>(bad_session).is_err());

    // Invalid variant
    let bad_variant = r#"{"id":"evt_x","type":"tui.toast.show","data":{"message":"test","variant":"invalid","duration":1000}}"#;
    assert!(serde_json::from_str::<Event<TuiToastShow>>(bad_variant).is_err());
}
