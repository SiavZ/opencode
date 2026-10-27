//! Purpose: list materialized legacy agents and their real permission rules.
//! Scope: one Location's config overlays and native agent catalog.
//! Uses: runtime/src/process_services.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: agent metadata is supplied by the runtime, never duplicated in CLI constants.
//! Contract tests: opencode/tests/admin/agents.rs

use anyhow::Context;
use opencode_runtime::Process;
use std::path::PathBuf;

pub async fn list(process: &Process, directory: PathBuf) -> anyhow::Result<()> {
    let agents = process
        .legacy_command(
            "app.agents",
            directory,
            Default::default(),
            Default::default(),
            None,
        )
        .await?;
    let mut agents = agents
        .as_array()
        .context("Invalid agent list response")?
        .iter()
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| {
        b["native"]
            .as_bool()
            .unwrap_or(false)
            .cmp(&a["native"].as_bool().unwrap_or(false))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    for agent in agents {
        println!(
            "{} ({})",
            agent["name"].as_str().context("Missing agent name")?,
            agent["mode"].as_str().context("Missing agent mode")?
        );
        println!("  {}", serde_json::to_string_pretty(&agent["permission"])?);
    }
    Ok(())
}
