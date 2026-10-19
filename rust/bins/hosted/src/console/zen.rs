//! Purpose: Zen and Go inference routes under /zen/v1 and /zen/go/v1.
//! Scope: stub created by the phase-12 parent, filled by child node p12.gateway.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! External:
//! |-- anyhow::Result
//! `-- axum::Router
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/state.rs
//! Invariants: owned by p12.gateway. Child modules may live in a sibling directory of the same name.
//! Contract tests: none yet.

use axum::Router;

use super::ConsoleState;

/// Route-module configuration read from the environment at startup. Owned by p12.gateway; add fields
/// here (secrets must stay out of `Debug`).
#[derive(Clone, Default)]
pub struct Config {}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config").finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::default())
    }
}

/// Routes owned by p12.gateway. Merged by `console::router`.
pub fn routes() -> Router<ConsoleState> {
    Router::new()
}
