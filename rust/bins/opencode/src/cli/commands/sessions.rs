//! Purpose: execute legacy Session administration through the composed domain service.
//! Scope: one local CLI invocation, preserving exported legacy JSON.
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/mod.rs
//! rust/crates/
//! `-- runtime/src/lib.rs
//! External:
//! |-- anyhow::{Context, bail}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/mod.rs
//! Invariants: file decoding and validation fail before durable import; no fabricated sessions.
//! Contract tests: opencode/tests/admin/mod.rs

use std::path::PathBuf;

use anyhow::{Context, bail};
use opencode_runtime::Process;
use serde_json::{Value, json};

use crate::cli::spec::{CommandId, Parsed};

pub async fn run(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    match parsed.command {
        CommandId::SessionList => {
            let limit = parsed.args.num("max-count");
            if limit.is_some_and(|value| !value.is_finite() || value < 0.0 || value.fract() != 0.0)
            {
                bail!("max-count must be a non-negative integer");
            }
            let mut query = json!({"roots": true});
            if let Some(limit) = limit {
                query["limit"] = json!(limit as u64);
            }
            let sessions = process
                .legacy_command(
                    "session.list",
                    directory,
                    Default::default(),
                    object(query),
                    None,
                )
                .await?;
            let sessions = sessions
                .as_array()
                .context("Invalid session list response")?;
            if sessions.is_empty() {
                return Ok(());
            }
            if parsed.args.str("format").as_deref() == Some("json") {
                let rows: Vec<_> = sessions.iter().map(|session| json!({
                    "id":session["id"], "title":session["title"],
                    "updated":session["time"]["updated"], "created":session["time"]["created"],
                    "projectId":session["projectID"], "directory":session["directory"]
                })).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            // Table date formatting and the platform pager need the locale service. Do not
            // silently substitute epoch timestamps for the baseline's local date strings.
            bail!("NotYetPorted: session list table locale and pager; use --format json");
        }
        CommandId::SessionDelete => {
            let id = parsed.args.str("sessionID").context("Missing sessionID")?;
            process
                .legacy_command(
                    "session.delete",
                    directory,
                    object(json!({"sessionID":id})),
                    Default::default(),
                    None,
                )
                .await?;
            println!("Session {id} deleted");
        }
        CommandId::Import => {
            let file = parsed.args.str("file").context("Missing file")?;
            if file.starts_with("http://") || file.starts_with("https://") {
                bail!("NotYetPorted: share URL import");
            }
            let path = directory.join(&file);
            let text = std::fs::read_to_string(path).map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => anyhow::anyhow!("File not found: {file}"),
                std::io::ErrorKind::PermissionDenied => {
                    anyhow::anyhow!("Failed to read file: Permission denied")
                }
                _ => anyhow::anyhow!("Failed to read file: {error}"),
            })?;
            let data: Value =
                serde_json::from_str(&text).with_context(|| format!("Invalid JSON in {file}"))?;
            let id = data
                .pointer("/info/id")
                .and_then(Value::as_str)
                .context("Invalid session data: missing info.id")?
                .to_owned();
            process
                .legacy_command(
                    "session.import",
                    directory,
                    Default::default(),
                    Default::default(),
                    Some(data),
                )
                .await?;
            println!("Imported session: {id}");
        }
        CommandId::Export => {
            let id = parsed
                .args
                .str("sessionID")
                .context("NotYetPorted: interactive export session picker; provide sessionID")?;
            eprintln!("Exporting session: {id}");
            let params = object(json!({"sessionID":id}));
            let info = process
                .legacy_command(
                    "session.get",
                    directory.clone(),
                    params.clone(),
                    Default::default(),
                    None,
                )
                .await
                .with_context(|| format!("Session not found: {id}"))?;
            let messages = process
                .legacy_command(
                    "session.messages",
                    directory,
                    params,
                    Default::default(),
                    None,
                )
                .await
                .with_context(|| format!("Session not found: {id}"))?;
            let mut data = json!({"info":info,"messages":messages});
            if parsed.args.bool("sanitize") {
                super::sanitize::sanitize(&mut data);
            }
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        _ => unreachable!("only implemented session commands are routed here"),
    }
    Ok(())
}

fn object(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().expect("literal object").clone()
}
