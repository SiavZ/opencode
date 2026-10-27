//! Purpose: run explicit database queries through the public runtime database service.
//! Scope: process database, including baseline write-capable SQL and ordered row formatting.
//! Uses: runtime/src/process_services.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: SQL errors propagate; TSV uses first-row keys and JS Array.join coercion.
//! Contract tests: opencode/tests/admin/database.rs

use crate::cli::spec::Parsed;
use anyhow::Context;
use opencode_runtime::Process;
use serde_json::Value;

pub async fn run(process: &Process, parsed: &Parsed) -> anyhow::Result<()> {
    let query = parsed.args.str("query").context("Missing SQL query")?;
    let rows = process.database_query(query).await?;
    if parsed.args.str("format").as_deref() == Some("json") {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let keys: Vec<_> = first
        .as_object()
        .context("Invalid database row")?
        .keys()
        .collect();
    println!(
        "{}",
        keys.iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>()
            .join("\t")
    );
    for row in &rows {
        println!(
            "{}",
            keys.iter()
                .map(|key| cell(&row[*key]))
                .collect::<Vec<_>>()
                .join("\t")
        );
    }
    Ok(())
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values.iter().map(cell).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
        value => value.to_string(),
    }
}
