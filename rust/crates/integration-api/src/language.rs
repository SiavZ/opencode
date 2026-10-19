//! Purpose: language-server domain contract: LSP config entries, status, symbol/range values,
//! diagnostics with baseline pretty/report formatting, and the `LanguageService` trait used by
//! edit/write tools, the lsp tool and the status route.
//! Scope: values and one async trait. Process and JSON-RPC code live in integration-adapters.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/diagnostics.rs
//! |-- integration-adapters/src/lsp/inventory.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/service.rs
//! |-- integration-adapters/tests/lsp_client.rs
//! |-- integration-adapters/tests/lsp_inventory.rs
//! |-- integration-adapters/tests/lsp_live.rs
//! |-- integration-adapters/tests/lsp_service.rs
//! `-- integration-api/src/lib.rs
//! Invariants: positions are zero-based UTF-16 code units (LSP), `report` matches
//! packages/opencode/src/lsp/diagnostic.ts byte for byte (20 errors per file, severity 1 only).
//! Contract tests: integration-api/src/language.rs (tests module)

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::IntegrationError;

pub const MAX_DIAGNOSTICS_PER_FILE: usize = 20;

/// Builtin server ids (packages/core/src/v1/config/lsp.ts:22-59). Order preserved.
pub const BUILTIN_SERVER_IDS: [&str; 38] = [
    "deno",
    "typescript",
    "vue",
    "eslint",
    "oxlint",
    "biome",
    "gopls",
    "ruby-lsp",
    "ty",
    "pyright",
    "elixir-ls",
    "zls",
    "csharp",
    "razor",
    "fsharp",
    "sourcekit-lsp",
    "rust",
    "clangd",
    "svelte",
    "astro",
    "jdtls",
    "kotlin-ls",
    "yaml-ls",
    "lua-ls",
    "php intelephense",
    "prisma",
    "dart",
    "ocaml-lsp",
    "bash",
    "terraform",
    "texlab",
    "dockerfile",
    "gleam",
    "clojure-lsp",
    "nixd",
    "tinymist",
    "haskell-language-server",
    "julials",
];

/// One `lsp.<id>` entry: `{disabled:true}` or a custom/override definition. Both baseline union
/// arms deserialize into this struct, `command` is empty for the disabled arm.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LspServerEntry {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization: Option<serde_json::Map<String, Value>>,
}

/// `ConfigLSPV1.Info`: unset/false disables, true enables builtins, object overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LspConfig {
    Enabled(bool),
    Servers(BTreeMap<String, LspServerEntry>),
}

impl LspConfig {
    /// Baseline filter `requiresExtensionsForCustomServers`.
    pub fn validate(&self) -> Result<(), IntegrationError> {
        let Self::Servers(map) = self else {
            return Ok(());
        };
        let ok = map.iter().all(|(id, e)| {
            e.disabled == Some(true)
                || BUILTIN_SERVER_IDS.contains(&id.as_str())
                || e.extensions.is_some()
        });
        if ok {
            return Ok(());
        }
        Err(IntegrationError::Validation(
            "For custom LSP servers, 'extensions' array is required.".into(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    /// UTF-16 code units.
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: u32,
    pub location: SymbolLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnySymbol {
    Document(DocumentSymbol),
    Workspace(Symbol),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connected,
    Error,
}

/// `LSPStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: String,
    pub name: String,
    pub root: String,
    pub status: ConnectionState,
}

/// LSP `Diagnostic` (vscode-languageserver-types). Extra fields preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Diagnostic {
    /// Baseline `Diagnostic.pretty`.
    pub fn pretty(&self) -> String {
        let severity = match self.severity.unwrap_or(1) {
            2 => "WARN",
            3 => "INFO",
            4 => "HINT",
            _ => "ERROR",
        };
        format!(
            "{severity} [{}:{}] {}",
            self.range.start.line + 1,
            self.range.start.character + 1,
            self.message
        )
    }
}

/// Baseline `Diagnostic.report`: empty string when the file has no severity-1 issues.
pub fn report(file: &str, issues: &[Diagnostic]) -> String {
    let errors: Vec<&Diagnostic> = issues.iter().filter(|d| d.severity == Some(1)).collect();
    if errors.is_empty() {
        return String::new();
    }
    let body = errors
        .iter()
        .take(MAX_DIAGNOSTICS_PER_FILE)
        .map(|d| d.pretty())
        .collect::<Vec<_>>()
        .join("\n");
    let more = errors.len().saturating_sub(MAX_DIAGNOSTICS_PER_FILE);
    let suffix = if more > 0 {
        format!("\n... and {more} more")
    } else {
        String::new()
    };
    format!("<diagnostics file=\"{file}\">\n{body}{suffix}\n</diagnostics>")
}

/// `touchFile(file, diagnostics?)` wait mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsWait {
    Document,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocInput {
    pub file: String,
    pub line: u32,
    pub character: u32,
}

/// Per-Location language service, mirrors `LSP.Interface` (packages/opencode/src/lsp/lsp.ts:119-134).
#[async_trait]
pub trait LanguageService: Send + Sync {
    async fn init(&self) -> Result<(), IntegrationError>;
    async fn status(&self) -> Vec<Status>;
    async fn has_clients(&self, file: &str) -> bool;
    async fn touch_file(
        &self,
        file: &str,
        wait: Option<DiagnosticsWait>,
    ) -> Result<(), IntegrationError>;
    async fn diagnostics(&self) -> BTreeMap<String, Vec<Diagnostic>>;
    async fn hover(&self, input: LocInput) -> Result<Value, IntegrationError>;
    async fn definition(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError>;
    async fn references(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError>;
    async fn implementation(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError>;
    async fn document_symbol(&self, uri: &str) -> Result<Vec<AnySymbol>, IntegrationError>;
    async fn workspace_symbol(&self, query: &str) -> Result<Vec<Symbol>, IntegrationError>;
    async fn prepare_call_hierarchy(&self, input: LocInput)
    -> Result<Vec<Value>, IntegrationError>;
    async fn incoming_calls(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError>;
    async fn outgoing_calls(&self, input: LocInput) -> Result<Vec<Value>, IntegrationError>;
    /// Stop every client and join its process (Location close).
    async fn shutdown(&self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diag(sev: u8, line: u32, msg: &str) -> Diagnostic {
        serde_json::from_value(json!({
            "range": {"start":{"line":line,"character":0},"end":{"line":line,"character":1}},
            "severity": sev, "message": msg
        }))
        .unwrap()
    }

    #[test]
    fn report_matches_baseline() {
        assert_eq!(report("a.ts", &[diag(2, 0, "w")]), "");
        let out = report("a.ts", &[diag(1, 4, "bad"), diag(2, 0, "w")]);
        assert_eq!(
            out,
            "<diagnostics file=\"a.ts\">\nERROR [5:1] bad\n</diagnostics>"
        );
        let many: Vec<_> = (0..23).map(|i| diag(1, i, "e")).collect();
        assert!(report("f", &many).ends_with("ERROR [20:1] e\n... and 3 more\n</diagnostics>"));
    }

    #[test]
    fn custom_server_requires_extensions() {
        let ok: LspConfig = serde_json::from_value(json!({"rust":{"command":["x"]}})).unwrap();
        assert!(ok.validate().is_ok());
        let bad: LspConfig = serde_json::from_value(json!({"mine":{"command":["x"]}})).unwrap();
        assert!(bad.validate().is_err());
        let off: LspConfig = serde_json::from_value(json!({"mine":{"disabled":true}})).unwrap();
        assert!(off.validate().is_ok());
        assert_eq!(
            serde_json::from_value::<LspConfig>(json!(true)).unwrap(),
            LspConfig::Enabled(true)
        );
    }
}
