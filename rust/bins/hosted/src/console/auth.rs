//! Purpose: console auth routes: /auth/authorize, /auth/*callback, /auth/status, /auth/logout.
//! Scope: stub created by the phase-12 parent, filled by child node p12.identity.
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
//! Invariants: owned by p12.identity. Child modules may live in a sibling directory of the same name.
//! Contract tests: none yet.

use axum::Router;

use super::ConsoleState;

/// Route-module configuration read from the environment at startup. Owned by p12.identity; add fields
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

/// Routes owned by p12.identity. Merged by `console::router`.
pub fn routes() -> Router<ConsoleState> {
    Router::new()
}
