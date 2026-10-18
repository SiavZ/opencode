//! Purpose: shared console route state: configuration and adapter handles injected into routes.
//! Scope: process lifetime, cloned per request.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! |-- hosted/src/console/billing.rs
//! |-- hosted/src/console/support.rs
//! |-- hosted/src/console/workspace.rs
//! `-- hosted/src/console/zen.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! `-- std::sync::Arc
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/mod.rs
//! Invariants: owned by the phase-12 parent. Each route module owns its own `Config` type, so
//! children never edit this file. Secrets never appear in Debug output.
//! Contract tests: none yet.

use std::sync::Arc;

use opencode_hosted_adapters::console::db::ConsoleDb;

// Fields are read by route modules as they land.
#[allow(dead_code)]
/// Console configuration. Mirrors the `Resource.*` values the baseline reads.
#[derive(Clone, Debug, Default)]
pub struct ConsoleConfig {
    /// `Resource.App.stage`, e.g. `production`, `dev`.
    pub stage: String,
    pub auth: super::auth::Config,
    pub workspace: super::workspace::Config,
    pub billing: super::billing::Config,
    pub zen: super::zen::Config,
    pub support: super::support::Config,
}

impl ConsoleConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            stage: std::env::var("OPENCODE_STAGE").unwrap_or_else(|_| "dev".into()),
            auth: super::auth::Config::from_env()?,
            workspace: super::workspace::Config::from_env()?,
            billing: super::billing::Config::from_env()?,
            zen: super::zen::Config::from_env()?,
            support: super::support::Config::from_env()?,
        })
    }
}

// Fields are read by route modules as they land.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ConsoleState {
    pub config: Arc<ConsoleConfig>,
    pub db: ConsoleDb,
}
