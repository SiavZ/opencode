//! Purpose: scoped tool registry (packages/core/src/tool/registry.ts): Location registrations stack
//! over process-level application tools, materialization snapshots advertised identities and
//! rejects stale calls, settlement converts failures and bounds output through a store port.
//! Scope: in-memory registry, no I/O except through `OutputBounder`.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/context.rs
//! |-- tool-api/src/definition.rs
//! `-- tool-api/src/output.rs
//! External:
//! `-- opencode_permissions::{Rule, wholly_disabled}
//! Used by:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! `-- tool-api/tests/registry.rs
//! Invariants: an in-flight call keeps its captured tool even after removal; a not-yet-started call
//! whose advertised registration was replaced or removed fails with `Stale tool call: <name>`.
//! Contract tests: tool-api/tests/registry.rs

use crate::context::ToolContext;
use crate::definition::{Tool, ToolDefinition, validate_name};
use crate::output::{ToolOutput, ToolResultValue};
use async_trait::async_trait;
use opencode_permissions::{Rule, wholly_disabled};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RegistrationError {
    pub name: String,
    pub message: String,
}

/// Result of bounding tool output (`ToolOutputStore.BoundResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct BoundOutput {
    pub output: ToolOutput,
    pub output_paths: Vec<String>,
}

/// Output bounding and retention port (`ToolOutputStore.bound`), implemented in `tools::output`.
#[async_trait]
pub trait OutputBounder: Send + Sync {
    async fn bound(
        &self,
        session_id: &str,
        tool_call_id: &str,
        output: ToolOutput,
    ) -> Result<BoundOutput, String>;
}

/// Pass-through bounder for tests and contexts with no retention.
#[derive(Debug, Default)]
pub struct PassthroughBounder;

#[async_trait]
impl OutputBounder for PassthroughBounder {
    async fn bound(
        &self,
        _session_id: &str,
        _tool_call_id: &str,
        output: ToolOutput,
    ) -> Result<BoundOutput, String> {
        Ok(BoundOutput {
            output,
            output_paths: vec![],
        })
    }
}

#[derive(Clone)]
struct Entry {
    identity: u64,
    token: u64,
    tool: Arc<dyn Tool>,
}

type Table = BTreeMap<String, Vec<Entry>>;

struct Inner {
    applications: Mutex<Table>,
    local: Mutex<Table>,
    next: AtomicU64,
    bounder: Arc<dyn OutputBounder>,
}

fn lock(table: &Mutex<Table>) -> MutexGuard<'_, Table> {
    table.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The registry. Cheap to clone.
#[derive(Clone)]
pub struct ToolRegistry {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry").finish_non_exhaustive()
    }
}

/// Scoped registration handle. Dropping it removes its tools (Effect scope finalizer equivalent).
pub struct Registration {
    inner: Arc<Inner>,
    token: u64,
    names: Vec<String>,
    application: bool,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let table = if self.application {
            &self.inner.applications
        } else {
            &self.inner.local
        };
        let mut table = lock(table);
        for name in &self.names {
            if let Some(entries) = table.get_mut(name) {
                entries.retain(|entry| entry.token != self.token);
                if entries.is_empty() {
                    table.remove(name);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecuteInput {
    pub name: String,
    pub input: Value,
}

/// `ToolRegistry.Settlement`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settlement {
    pub result: ToolResultValue,
    pub output: Option<ToolOutput>,
    pub output_paths: Vec<String>,
}

impl Settlement {
    fn error(message: String) -> Self {
        Self {
            result: ToolResultValue::error(message),
            output: None,
            output_paths: vec![],
        }
    }
}

/// Immutable advertisement snapshot.
pub struct Materialization {
    registry: ToolRegistry,
    entries: Vec<(String, u64, Arc<dyn Tool>)>,
}

impl std::fmt::Debug for Materialization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Materialization")
            .field("tools", &self.names())
            .finish()
    }
}

impl ToolRegistry {
    pub fn new(bounder: Arc<dyn OutputBounder>) -> Self {
        Self {
            inner: Arc::new(Inner {
                applications: Mutex::new(Table::new()),
                local: Mutex::new(Table::new()),
                next: AtomicU64::new(1),
                bounder,
            }),
        }
    }

    fn insert(
        &self,
        tools: Vec<(String, Arc<dyn Tool>)>,
        application: bool,
    ) -> Result<Registration, RegistrationError> {
        if let Some((name, _)) = tools.iter().find(|(name, _)| !validate_name(name)) {
            return Err(RegistrationError {
                name: name.clone(),
                message: format!("Invalid tool name: {name}"),
            });
        }
        let token = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let table = if application {
            &self.inner.applications
        } else {
            &self.inner.local
        };
        let mut table = lock(table);
        let names = tools.iter().map(|(name, _)| name.clone()).collect();
        for (name, tool) in tools {
            let identity = self.inner.next.fetch_add(1, Ordering::Relaxed);
            table.entry(name).or_default().push(Entry {
                identity,
                token,
                tool,
            });
        }
        Ok(Registration {
            inner: self.inner.clone(),
            token,
            names,
            application,
        })
    }

    /// Location-scoped registration (`Tools.Service.register`). Later registrations shadow earlier.
    pub fn register(
        &self,
        tools: Vec<(String, Arc<dyn Tool>)>,
    ) -> Result<Registration, RegistrationError> {
        self.insert(tools, false)
    }

    /// Process-level application registration (`ApplicationTools`). Location tools override them.
    pub fn register_application(
        &self,
        tools: Vec<(String, Arc<dyn Tool>)>,
    ) -> Result<Registration, RegistrationError> {
        self.insert(tools, true)
    }

    fn current(&self, name: &str) -> Option<Entry> {
        lock(&self.inner.local)
            .get(name)
            .and_then(|entries| entries.last().cloned())
            .or_else(|| {
                lock(&self.inner.applications)
                    .get(name)
                    .and_then(|entries| entries.last().cloned())
            })
    }

    /// Snapshot the visible tools, hiding those wholly denied by `permissions`.
    pub fn materialize(&self, permissions: &[Rule]) -> Materialization {
        let mut visible: BTreeMap<String, Entry> = BTreeMap::new();
        for (name, entries) in lock(&self.inner.applications).iter() {
            if let Some(entry) = entries.last() {
                visible.insert(name.clone(), entry.clone());
            }
        }
        // Location registrations override application tools of the same name.
        for (name, entries) in lock(&self.inner.local).iter() {
            if let Some(entry) = entries.last() {
                visible.insert(name.clone(), entry.clone());
            }
        }
        let entries = visible
            .into_iter()
            .filter(|(name, entry)| {
                !wholly_disabled(entry.tool.permission().unwrap_or(name), permissions)
            })
            .map(|(name, entry)| (name, entry.identity, entry.tool))
            .collect();
        Materialization {
            registry: self.clone(),
            entries,
        }
    }
}

impl Materialization {
    pub fn names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.entries
            .iter()
            .map(|(name, _, tool)| tool.definition(name))
            .collect()
    }

    /// Settle one call against this advertisement.
    pub async fn settle(
        &self,
        call: ExecuteInput,
        context: &ToolContext,
    ) -> Result<Settlement, String> {
        let Some((_, advertised, _)) = self.entries.iter().find(|(name, _, _)| *name == call.name)
        else {
            return Ok(Settlement::error(format!("Unknown tool: {}", call.name)));
        };
        let Some(current) = self.registry.current(&call.name) else {
            return Ok(Settlement::error(format!("Stale tool call: {}", call.name)));
        };
        if current.identity != *advertised {
            return Ok(Settlement::error(format!("Stale tool call: {}", call.name)));
        }
        // The captured Arc keeps the tool alive even if its registration is dropped mid-flight.
        let output = match current.tool.execute(call.input, context).await {
            Ok(output) => output,
            Err(failure) => return Ok(Settlement::error(failure.message)),
        };
        let bounded = self
            .registry
            .inner
            .bounder
            .bound(&context.session_id, &context.tool_call_id, output)
            .await?;
        let result = bounded.output.to_result_value();
        let output = (!matches!(result, ToolResultValue::Error { .. })).then_some(bounded.output);
        Ok(Settlement {
            result,
            output,
            output_paths: bounded.output_paths,
        })
    }
}
