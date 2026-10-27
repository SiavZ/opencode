//! Purpose: list real global provider credentials and remove an explicitly selected credential.
//! Scope: process-global auth/catalog services, without booting a project Location.
//! Uses: runtime/src/process_services.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: credential values and environment values are never printed.
//! Contract tests: opencode/tests/admin/providers.rs

use crate::cli::spec::{CommandId, Parsed};
use anyhow::Context;
use opencode_runtime::Process;
use serde_json::Value;

pub async fn run(process: &Process, parsed: &Parsed) -> anyhow::Result<()> {
    let credentials = process.global_auth_list().await?;
    let credentials = credentials
        .as_object()
        .context("Invalid credential store")?;
    if parsed.command == CommandId::ProvidersLogout && credentials.is_empty() {
        println!("\n┌  Remove credential\n│\n■  No credentials found");
        return Ok(());
    }
    let catalog = process.models_catalog().await?;
    if parsed.command == CommandId::ProvidersLogout {
        println!("\n┌  Remove credential");
        let selected = parsed.args.str("provider").context("Missing provider")?;
        let id = credentials
            .keys()
            .find(|id| {
                **id == selected
                    || catalog[*id]["name"]
                        .as_str()
                        .is_some_and(|name| name.to_lowercase() == selected.to_lowercase())
            })
            .with_context(|| format!("Unknown configured provider \"{selected}\""))?;
        process.global_auth_remove(id).await?;
        println!("│\n└  Logout successful");
        return Ok(());
    }
    let path = process.options.paths.data.join("auth.json");
    let display = path
        .strip_prefix(&process.options.paths.home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string());
    println!("\n┌  Credentials {display}");
    for (id, credential) in credentials {
        let name = catalog[id]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        println!(
            "│\n●  {name} {}",
            credential["type"]
                .as_str()
                .context("Invalid credential type")?
        );
    }
    println!("│\n└  {} credentials", credentials.len());
    let active: Vec<_> = catalog
        .as_object()
        .context("Invalid model catalog")?
        .iter()
        .flat_map(|(id, provider)| {
            provider["env"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|name| {
                    process
                        .options
                        .environment
                        .var(name)
                        .is_some_and(|value| !value.is_empty())
                })
                .map(move |name| {
                    (
                        provider["name"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .unwrap_or(id),
                        name,
                    )
                })
        })
        .collect();
    if !active.is_empty() {
        println!("\n┌  Environment");
        for (provider, name) in &active {
            println!("│\n●  {provider} {name}");
        }
        println!(
            "│\n└  {} environment variable{}",
            active.len(),
            if active.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}
