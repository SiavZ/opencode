//! Purpose: `opencode-hosted` composition root for hosted HTTP services. Serves the console
//! router (Phase 12); Phase 13 merges sharing/analytics/sites routers here.
//! Scope: process entrypoint.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/sharing/mod.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- axum::serve
//! |-- tokio::main
//! `-- tokio::net::TcpListener::bind
//! Used by:
//! `-- none, entrypoint
//! Invariants: never deploys; binds only the address given in OPENCODE_HOSTED_ADDR
//! (default 127.0.0.1:8787); requires OPENCODE_CONSOLE_DATABASE_URL.
//! Contract tests: none yet.

use std::sync::Arc;

use anyhow::Context;

mod console;
mod sharing;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Phase 13: the enterprise share API and the public share API are separate Workers in the
    // baseline (opncd.ai and api.<domain>), so they are separate serve modes, not merged routes.
    match std::env::var("OPENCODE_HOSTED_SERVICE").as_deref() {
        Ok("enterprise-share") => return sharing::serve_enterprise().await,
        Ok("public-share") => return sharing::serve_public().await,
        _ => {}
    }
    let url = std::env::var("OPENCODE_CONSOLE_DATABASE_URL")
        .context("OPENCODE_CONSOLE_DATABASE_URL is required")?;
    let state = console::ConsoleState {
        config: Arc::new(console::state::ConsoleConfig::from_env()?),
        db: opencode_hosted_adapters::console::db::ConsoleDb::connect(&url)?,
    };
    let addr = std::env::var("OPENCODE_HOSTED_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, console::router(state)).await?;
    Ok(())
}
