//! Purpose: inspect real Location MCP and skill services from the CLI.
//! Scope: one composed Location, including actual transport status and stored OAuth state.
//! Uses: runtime/src/process_services.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: displayed states come from integrations, never inferred from configuration alone.
//! Contract tests: opencode/tests/admin/integrations.rs

use crate::cli::spec::{CommandId, Parsed};
use anyhow::Context;
use opencode_runtime::Process;
use serde_json::Value;
use std::path::PathBuf;

async fn logout(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    println!("\n┌  MCP OAuth Logout");
    let credentials = process
        .legacy_command(
            "mcp.auth.list",
            directory.clone(),
            Default::default(),
            Default::default(),
            None,
        )
        .await?;
    let credentials = credentials
        .as_object()
        .context("Invalid MCP credential inventory")?;
    if credentials.is_empty() {
        println!("│\n▲  No MCP OAuth credentials stored\n│\n└  Done");
        return Ok(());
    }
    let name = parsed.args.str("name").context("Missing MCP server name")?;
    if !credentials.contains_key(&name) {
        println!("│\n■  No credentials found for: {name}\n│\n└  Done");
        return Ok(());
    }
    process
        .legacy_command(
            "mcp.auth.remove",
            directory,
            serde_json::json!({"name":name})
                .as_object()
                .expect("literal object")
                .clone(),
            Default::default(),
            None,
        )
        .await?;
    println!("│\n◆  Removed OAuth credentials for {name}\n│\n└  Done");
    Ok(())
}

pub async fn run(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    if parsed.command == CommandId::McpLogout {
        return logout(process, parsed, directory).await;
    }
    if parsed.command == CommandId::DebugSkill {
        let skills = process
            .legacy_command(
                "app.skills",
                directory,
                Default::default(),
                Default::default(),
                None,
            )
            .await?;
        println!("{}", serde_json::to_string_pretty(&skills)?);
        return Ok(());
    }
    let config = process.resolved_config(directory.clone()).await?;
    let state = process
        .legacy_command(
            "mcp.cli.list",
            directory,
            Default::default(),
            Default::default(),
            None,
        )
        .await?;
    let auth = parsed.command == CommandId::McpAuthList;
    let servers: Vec<_> = config
        .get("mcp")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|servers| servers.iter())
        .filter(|(_, config)| {
            config.get("type").is_some()
                && (!auth
                    || config["type"] == "remote"
                        && config.get("oauth") != Some(&Value::Bool(false)))
        })
        .collect();
    println!(
        "\n┌  {}",
        if auth {
            "MCP OAuth Status"
        } else {
            "MCP Servers"
        }
    );
    if servers.is_empty() {
        println!(
            "│\n▲  {}",
            if auth {
                "No OAuth-capable MCP servers configured"
            } else {
                "No MCP servers configured"
            }
        );
        println!(
            "│\n└  {}",
            if auth {
                "Done"
            } else {
                "Add servers with: opencode mcp add"
            }
        );
        return Ok(());
    }
    for (name, config) in &servers {
        let (icon, status) = if auth {
            match state["auth"][name.as_str()]
                .as_str()
                .context("Invalid MCP OAuth state")?
            {
                "authenticated" => ("✓", "authenticated"),
                "expired" => ("⚠", "expired"),
                "not_authenticated" => ("✗", "not authenticated"),
                other => anyhow::bail!("Invalid MCP OAuth state: {other}"),
            }
        } else {
            match state["statuses"][name.as_str()]["status"].as_str() {
                None => ("○", "not initialized"),
                Some("connected") => ("✓", "connected"),
                Some("disabled") => ("○", "disabled"),
                Some("needs_auth") => ("⚠", "needs authentication"),
                Some("needs_client_registration") => ("✗", "needs client registration"),
                Some("failed") => ("✗", "failed"),
                Some(other) => anyhow::bail!("Invalid MCP transport state: {other}"),
            }
        };
        let oauth = !auth
            && status == "connected"
            && config["type"] == "remote"
            && config
                .get("oauth")
                .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
            && state["stored"][name.as_str()] == true;
        println!(
            "│\n●  {icon} {name} {status}{}",
            if oauth { " (OAuth)" } else { "" }
        );
        if !auth && let Some(error) = state["statuses"][name.as_str()]["error"].as_str() {
            println!("│      {error}");
        }
        let hint = if config["type"] == "remote" {
            config["url"]
                .as_str()
                .context("Invalid MCP server URL")?
                .to_owned()
        } else {
            config["command"]
                .as_array()
                .context("Invalid MCP server command")?
                .iter()
                .map(|value| value.as_str().context("Invalid MCP command argument"))
                .collect::<anyhow::Result<Vec<_>>>()?
                .join(" ")
        };
        println!("│      {hint}");
    }
    println!(
        "│\n└  {} {}server(s)",
        servers.len(),
        if auth { "OAuth-capable " } else { "" }
    );
    Ok(())
}
