//! Purpose: retain process application tool registrations for an embedded host's lifetime.
//! Scope: synchronous validated registration over the same registry used by Location runners.
//! Uses:
//! rust/crates/
//! |-- sdk-embedded/src/embedded.rs
//! `-- tool-api/src/lib.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- sdk-embedded/src/embedded.rs
//! Invariants: registrations survive ignored return values; closing rejects later registrations.
//! Contract tests: sdk-embedded/tests/embedded.rs

use std::sync::Arc;

use opencode_tool_api::{RegistrationError, Tool, ToolRegistry};

use super::Lifetime;

#[derive(Clone)]
pub struct ApplicationTools {
    registry: ToolRegistry,
    lifetime: Arc<Lifetime>,
}

impl ApplicationTools {
    pub(super) fn new(registry: ToolRegistry, lifetime: Arc<Lifetime>) -> Self {
        Self { registry, lifetime }
    }

    /// Register process-wide tools. Location-owned definitions retain precedence.
    pub fn register(&self, tools: Vec<(String, Arc<dyn Tool>)>) -> Result<(), RegistrationError> {
        let mut registrations = self
            .lifetime
            .registrations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.lifetime.cancel.is_cancelled() {
            return Err(RegistrationError {
                name: String::new(),
                message: "embedded host is closed".into(),
            });
        }
        registrations.push(self.registry.register_application(tools)?);
        Ok(())
    }
}
