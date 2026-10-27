//! Purpose: console HTTP routes (axum) composed by the hosted binary. Each child route module
//! exposes `pub fn routes() -> Router<ConsoleState>` and is merged here.
//! Scope: module root and router composition.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/billing.rs
//! |-- hosted/src/console/state.rs
//! |-- hosted/src/console/support.rs
//! |-- hosted/src/console/workspace.rs
//! `-- hosted/src/console/zen.rs
//! External:
//! `-- axum::Router
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/billing.rs
//! |-- hosted/src/console/support.rs
//! |-- hosted/src/console/workspace.rs
//! |-- hosted/src/console/zen.rs
//! `-- hosted/src/main.rs
//! Invariants: owned by phase-12; route modules call hosted-domain/hosted-adapters, never raw SQL.
//! Contract tests: route unit tests in each module (tower oneshot).

pub mod auth;
pub mod billing;
pub mod browser;
pub mod issuer;
pub mod public;
pub mod referral;
pub mod state;
pub mod support;
pub mod workers;
pub mod workspace;
pub mod zen;

#[cfg(test)]
mod tests;
use axum::Router;

pub use state::ConsoleState;

/// Console router: every route module's `routes()` merged over shared state.
pub fn router(state: ConsoleState) -> Router {
    Router::new()
        .merge(auth::routes())
        .merge(workspace::routes())
        .merge(billing::routes())
        .merge(browser::routes())
        .merge(public::routes())
        .merge(referral::routes())
        .merge(zen::routes())
        .merge(support::routes())
        .with_state(state)
}
