//! Purpose: compare baseline HTTP, tool, provider, CLI and event registries with coverage inventory.
//! Scope: read-only build tooling; never infers coverage approval from implementation.
//! Uses:
//! rust/xtask/
//! `-- src/workspace.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- hex::encode
//! |-- regex::Regex::new
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Value
//! |-- serde_json::Value::as_str
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- sha2::Digest
//! |-- sha2::Sha256::digest
//! |-- toml::from_str
//! `-- walkdir::WalkDir::new
//! Used by:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/lib.rs
//! |-- src/main.rs
//! `-- tests/gates.rs
//! Invariants: missing inventory, uncovered discoveries, stale rows and unknown coverage IDs fail.
//! Contract tests: xtask/tests/gates.rs

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub source: String,
    pub line: usize,
    pub coverage: String,
    pub status: String,
}
#[derive(Default, Deserialize, Serialize)]
struct Inventory {
    #[serde(default)]
    entry: Vec<Entry>,
}

pub fn run(args: &[String]) -> Result<()> {
    if args == ["--write"] {
        return write_inventory(&crate::workspace::rust_root()?);
    }
    if !args.is_empty() {
        bail!("usage: inventory [--write]");
    }
    let root = crate::workspace::rust_root()?;
    crate::workspace::finish("inventory", validate(&root)?)
}

pub fn discover(repo: &Path) -> Result<BTreeSet<(String, String, String, usize)>> {
    let mut found = BTreeSet::new();
    // Keep both current and legacy producers: neither registry is a replacement for the other.
    let patterns = [
        (
            "tool",
            regex::Regex::new(r#"Tool\.define(?:<[^;]+?>)?\(\s*["']([^"']+)["']"#)?,
        ),
        (
            "tool",
            regex::Regex::new(r#"export const name\s*=\s*["']([^"']+)["']"#)?,
        ),
        ("cli", regex::Regex::new(r#"command:\s*["']([^"']+)["']"#)?),
        (
            "http-declaration",
            regex::Regex::new(
                r#"HttpApiEndpoint\.(?:get|post|put|patch|delete|head|options)\(\s*["']([^"']+)["']"#,
            )?,
        ),
        (
            "event",
            regex::Regex::new(
                r#"(?:Event|BusEvent)\.define\(\s*(?:\{\s*type:\s*)?["']([^"']+)["']"#,
            )?,
        ),
    ];
    for directory in [
        "packages/protocol/src",
        "packages/schema/src",
        "packages/core/src",
        "packages/opencode/src",
    ] {
        for entry in walkdir::WalkDir::new(repo.join(directory)) {
            let entry = entry?;
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "ts")
            {
                continue;
            }
            let source = entry
                .path()
                .strip_prefix(repo)?
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(entry.path())?;
            for (kind, pattern) in &patterns {
                if (*kind == "tool" && !source.contains("/tool/"))
                    || (*kind == "cli" && !source.contains("/cli/"))
                {
                    continue;
                }
                for capture in pattern.captures_iter(&text) {
                    let hit = capture.get(1).context("registry capture")?;
                    found.insert((
                        kind.to_string(),
                        hit.as_str().into(),
                        source.clone(),
                        text[..hit.start()].lines().count().max(1),
                    ));
                }
            }
        }
    }
    let path = "packages/sdk/openapi.json";
    let text = std::fs::read_to_string(repo.join(path))?;
    let api: serde_json::Value = serde_json::from_str(&text)?;
    for (route, methods) in api["paths"].as_object().context("OpenAPI paths missing")? {
        for (method, operation) in methods.as_object().context("OpenAPI path item")? {
            if !matches!(
                method.as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
            ) {
                continue;
            }
            let id = operation["operationId"]
                .as_str()
                .context("OpenAPI operationId missing")?;
            let marker = format!("\"operationId\": \"{id}\"");
            let line = text
                .find(&marker)
                .map(|at| text[..at].lines().count() + 1)
                .context("OpenAPI source operationId")?;
            found.insert((
                "http".into(),
                format!("{} {route} ({id})", method.to_uppercase()),
                path.into(),
                line,
            ));
        }
    }
    let source = "packages/opencode/src/provider/provider.ts";
    let text = std::fs::read_to_string(repo.join(source))?;
    let start = text
        .find("const BUNDLED_PROVIDERS")
        .context("bundled provider registry missing")?;
    let end = text[start..]
        .find("\n}")
        .context("bundled provider registry terminator missing")?
        + start;
    for capture in
        regex::Regex::new(r#"(?m)^\s*["']([^"']+)["']\s*:"#)?.captures_iter(&text[start..end])
    {
        let hit = capture.get(1).context("provider capture")?;
        found.insert((
            "provider".into(),
            hit.as_str().into(),
            source.into(),
            text[..start + hit.start()].lines().count().max(1),
        ));
    }
    for kind in ["http", "tool", "provider", "cli", "event"] {
        if !found.iter().any(|row| row.0 == kind) {
            bail!(
                "{kind} baseline extraction produced zero entries; registry syntax or source layout changed"
            );
        }
    }
    Ok(found)
}

pub fn compare(
    found: &BTreeSet<(String, String, String, usize)>,
    entries: &[Entry],
    coverage: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        let key = (
            entry.kind.clone(),
            entry.name.clone(),
            entry.source.clone(),
            entry.line,
        );
        if !seen.insert(key.clone()) {
            errors.push(format!("duplicate inventory row {key:?}"));
        }
        if !found.contains(&key) {
            errors.push(format!("stale inventory row {key:?}"));
        }
        if !matches!(
            entry.status.as_str(),
            "unverified" | "not-ported" | "fixture-only"
        ) {
            errors.push(format!("{} {}: inventory status {:?} needs explicit evidence/review, not generated approval", entry.kind, entry.name, entry.status));
        }
        if entry.coverage.is_empty()
            || !coverage
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .any(|id| id == entry.coverage)
        {
            errors.push(format!(
                "{} {}: unknown coverage ID {:?}",
                entry.kind, entry.name, entry.coverage
            ));
        }
    }
    for row in found.difference(&seen) {
        errors.push(format!(
            "unmatched baseline {} {:?} at {}:{}",
            row.0, row.1, row.2, row.3
        ));
    }
    errors
}

/// Explicit baseline capture only. New rows never claim implementation approval.
pub fn write_inventory(root: &Path) -> Result<()> {
    let repo = crate::workspace::repo_root(root)?;
    let found = discover(&repo)?;
    let ledger_path = root.join("crates/server/tests/fixtures/replay-ledger.json");
    let ledger: serde_json::Value = if ledger_path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(ledger_path)?)?
    } else {
        serde_json::json!({})
    };
    let entry = found
        .into_iter()
        .map(|(kind, name, source, line)| {
            let digest = hex::encode(sha2::Sha256::digest(format!(
                "{kind}\0{name}\0{source}\0{line}"
            )));
            let coverage = format!("BASELINE-{}-{}", kind.to_uppercase(), &digest[..12]);
            let operation = name
                .rsplit_once('(')
                .map(|(_, id)| id.trim_end_matches(')'));
            let unported = kind == "http"
                && operation.is_some_and(|id| {
                    ledger
                        .get(format!("{id}#ok"))
                        .and_then(|v| v.get("category"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|category| matches!(category, "not-mounted" | "not-ported"))
                });
            Entry {
                kind,
                name,
                source,
                line,
                coverage,
                status: if unported { "not-ported" } else { "unverified" }.into(),
            }
        })
        .collect::<Vec<_>>();
    let mut coverage = String::from(
        "# Rust port baseline coverage inventory\n\nThis is a baseline registry inventory, not implementation approval.\n`unverified` means no end-to-end parity claim has been established for that row.\n`not-ported` means its happy-path HTTP case is recorded as `not-mounted` or\n`not-ported` in the versioned server replay ledger. The ledger can become stale\nas implementation changes; differential replay must validate it independently.\nNo row here is a release-readiness signoff. CLI parsing/help does not establish\ncommand execution, and provider registration does not establish a live model turn.\n\nThe inventory includes exported OpenAPI operations, current and legacy TypeScript\nHTTP declarations, built-in tool definitions, CLI command literals, event\ndefinitions and bundled provider adapters. Dynamic user/plugin/MCP registrations\nare outside the static baseline, not counted as implemented. HTTP declarations\nare retained separately so SDK generation cannot conceal a new TS route.\n\nRegenerate the baseline map explicitly with `cargo xtask inventory --write`.\nThe read-only `inventory` gate rejects missing, new or stale rows and unknown IDs.\nGenerated IDs identify source declarations; their presence proves enumeration only.\n\n| Coverage ID | Registry | Name | Baseline source | Status |\n| --- | --- | --- | --- | --- |\n",
    );
    for row in &entry {
        coverage.push_str(&format!(
            "| {} | {} | `{}` | `{}:{}` | {} |\n",
            row.coverage,
            row.kind,
            row.name.replace('|', "\\|"),
            row.source,
            row.line,
            row.status
        ));
    }
    let inventory = root.join("contracts/phase-01/inventory.toml");
    let docs = repo.join("docs/plans/rustport/coverage.md");
    std::fs::create_dir_all(inventory.parent().context("inventory parent")?)?;
    std::fs::create_dir_all(docs.parent().context("coverage parent")?)?;
    std::fs::write(
        inventory,
        format!(
            "# Captured baseline registry declarations. Status is NOT implementation approval.\n{}",
            toml::to_string_pretty(&Inventory { entry })?
        ),
    )?;
    std::fs::write(docs, coverage)?;
    Ok(())
}

pub fn validate(root: &Path) -> Result<Vec<String>> {
    let repo = crate::workspace::repo_root(root)?;
    let found = discover(&repo)?;
    let inventory = root.join("contracts/phase-01/inventory.toml");
    let coverage_path = repo.join("docs/plans/rustport/coverage.md");
    let mut errors = Vec::new();
    let entries: Inventory = if inventory.is_file() {
        toml::from_str(&std::fs::read_to_string(&inventory)?)?
    } else {
        errors.push(format!(
            "missing {} (reviewed registry coverage mappings required)",
            inventory.display()
        ));
        Inventory::default()
    };
    let coverage = if coverage_path.is_file() {
        std::fs::read_to_string(&coverage_path)?
    } else {
        errors.push(format!(
            "missing {} (coverage ID authority)",
            coverage_path.display()
        ));
        String::new()
    };
    errors.extend(compare(&found, &entries.entry, &coverage));
    println!(
        "inventory: {} baseline registry entries, {} mapped entries",
        found.len(),
        entries.entry.len()
    );
    Ok(errors)
}
