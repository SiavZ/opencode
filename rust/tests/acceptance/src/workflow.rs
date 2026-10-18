//! Purpose: typed scripted acceptance workflow format (TOML) and its evidence metadata: steps to run
//! a CLI, spawn `serve`, send HTTP requests, subscribe to SSE and assert status/body/events.
//! Scope: Phase 15 acceptance harness data model only, no execution.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde_json::Value
//! `-- toml::from_str
//! Used by:
//! rust/tests/
//! |-- acceptance/src/lib.rs
//! |-- acceptance/src/runner.rs
//! `-- acceptance/tests/workflows.rs
//! Invariants: unknown fields are rejected so typos in expectations never silently pass.
//! Contract tests: acceptance/tests/workflows.rs

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    /// Stable workflow name, used in the evidence `workflow` field.
    pub id: String,
    /// Coverage ID from docs/plans/rustport/coverage.md (e.g. PKG-04).
    pub requirement: String,
    /// Baseline file(s) or contract the expectations were traced from.
    pub baseline_source: String,
    /// Owning phase/module.
    pub owner: String,
    pub producer: String,
    pub consumer: String,
    /// `fixture` or `live`.
    pub classification: String,
    #[serde(rename = "step")]
    pub steps: Vec<Step>,
}

// Parsed once per workflow file, size of the HTTP variant is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Run the target with arguments to completion.
    Cli {
        args: Vec<String>,
        #[serde(default)]
        expect_exit: Option<i32>,
        #[serde(default)]
        expect_stdout_regex: Option<String>,
    },
    /// Start `serve --port <free> --pure` and wait for `GET /global/health` == 200.
    Serve {
        #[serde(default = "default_ready_secs")]
        ready_timeout_secs: u64,
    },
    /// Send one HTTP request to the running server.
    Http {
        method: String,
        path: String,
        #[serde(default)]
        body: Option<Value>,
        #[serde(default)]
        expect_status: Option<u16>,
        /// JSON pointer -> exact expected value (strings are templated).
        #[serde(default)]
        expect_json: Map<String, Value>,
        /// JSON pointer -> JSON type name (`string`, `number`, `object`, `array`, `bool`, `null`).
        #[serde(default)]
        expect_types: Map<String, Value>,
        /// The array at `array_pointer` (default root) has an item matching all pointer -> value pairs.
        #[serde(default)]
        expect_array_item: Map<String, Value>,
        #[serde(default)]
        array_pointer: String,
        /// Variable name -> JSON pointer whose string value is captured for later templating.
        #[serde(default)]
        capture: Map<String, Value>,
    },
    /// Open an SSE subscription named `name` and wait for its first event of type `first_type`.
    SseConnect {
        name: String,
        path: String,
        #[serde(default)]
        first_type: Option<String>,
    },
    /// Expect an event on stream `name` with `type` and matching pointer -> value pairs.
    SseExpect {
        name: String,
        r#type: String,
        #[serde(default)]
        expect_json: Map<String, Value>,
        #[serde(default = "default_event_secs")]
        timeout_secs: u64,
    },
}

fn default_ready_secs() -> u64 {
    60
}

fn default_event_secs() -> u64 {
    10
}

impl Workflow {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Load every `*.toml` in `dir`, sorted by file name for deterministic run order.
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        paths.iter().map(|p| Self::load(p)).collect()
    }

    /// Human-readable list of the expectations, recorded in the evidence `expected` field.
    pub fn expected(&self) -> Vec<String> {
        self.steps.iter().map(Step::describe).collect()
    }
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::Cli {
                args,
                expect_exit,
                expect_stdout_regex,
            } => format!(
                "cli {args:?} exit={} stdout~{}",
                expect_exit.map_or("any".into(), |c| c.to_string()),
                expect_stdout_regex.as_deref().unwrap_or("*")
            ),
            Step::Serve { .. } => "serve ready: GET /global/health 200".into(),
            Step::Http {
                method,
                path,
                expect_status,
                expect_json,
                expect_types,
                expect_array_item,
                ..
            } => format!(
                "{method} {path} status={} json={} types={} array_item={}",
                expect_status.map_or("any".into(), |s| s.to_string()),
                Value::Object(expect_json.clone()),
                Value::Object(expect_types.clone()),
                Value::Object(expect_array_item.clone())
            ),
            Step::SseConnect {
                name,
                path,
                first_type,
            } => format!(
                "sse {name} {path} first={}",
                first_type.as_deref().unwrap_or("*")
            ),
            Step::SseExpect {
                name,
                r#type,
                expect_json,
                ..
            } => format!(
                "sse {name} event {type} json={}",
                Value::Object(expect_json.clone()),
                type = r#type
            ),
        }
    }
}
