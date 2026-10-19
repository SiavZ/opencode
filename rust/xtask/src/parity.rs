//! Purpose: enumerate baseline route debt with exact categories and accountable domain owners.
//! Scope: read-only offline census, never fixture blessing or implementation approval.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- hex::encode
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- sha2::Digest
//! `-- sha2::Sha256
//! Used by:
//! rust/xtask/
//! |-- src/lib.rs
//! |-- src/main.rs
//! |-- tests/parity.rs
//! `-- tests/parity_owners.rs
//! Invariants: every fixture and ledger entry is counted once; absent-host debt is not a wire verdict.
//! Contract tests: xtask/tests/parity.rs

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

pub fn run(args: &[String]) -> Result<()> {
    if args.len() > 1
        || args.first().is_some_and(|owner| {
            ![
                "api",
                "session",
                "native",
                "integration",
                "provider",
                "cli",
                "composition",
            ]
            .contains(&owner.as_str())
        })
    {
        bail!(
            "usage: parity-census [api|session|native|integration|provider|cli|composition] > report.md; never changes fixtures"
        );
    }
    print!(
        "{}",
        render_for(
            &crate::workspace::rust_root()?,
            args.first().map(String::as_str)
        )?
    );
    Ok(())
}

/// Assign the domain implementation, with API boundary review required for every route.
pub fn owner(path: &str) -> &'static str {
    let path = path
        .strip_prefix("/api/")
        .unwrap_or(path.trim_start_matches('/'));
    match path.split('/').next().unwrap_or_default() {
        "session" => "session/cat",
        "pty" | "permission" | "question" | "file" | "find" | "fs" => "native/chicken",
        "mcp" | "lsp" | "formatter" | "skill" | "integration" | "reference" | "command" => {
            "integration/chick"
        }
        "provider" | "auth" | "credential" | "model" => "provider/bird",
        "tui" => "CLI/evergreen + TUI/herb",
        "config" | "global" | "path" | "project" | "instance" | "vcs" | "location" | "agent"
        | "experimental" | "sync" => "composition/iwazaru",
        _ => "API/bat",
    }
}

pub fn render(root: &Path) -> Result<String> {
    render_for(root, None)
}

pub fn render_for(root: &Path, filter: Option<&str>) -> Result<String> {
    let ledger_text =
        std::fs::read_to_string(root.join("crates/server/tests/fixtures/replay-ledger.json"))?;
    let ledger_hash = hex::encode(Sha256::digest(ledger_text.as_bytes()));
    let ledger: Value = serde_json::from_str(&ledger_text)?;
    let ledger = ledger
        .as_object()
        .context("replay ledger must be an object")?;
    let mut files = std::fs::read_dir(root.join("crates/server/tests/fixtures/baseline"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "json"));
    files.sort();
    let mut cases = Vec::new();
    let mut corpus_hash = Sha256::new();
    for file in files {
        let text = std::fs::read_to_string(&file)?;
        corpus_hash.update(
            file.file_name()
                .context("missing fixture filename")?
                .as_encoded_bytes(),
        );
        corpus_hash.update([0]);
        corpus_hash.update(text.as_bytes());
        corpus_hash.update([0]);
        let family: Value = serde_json::from_str(&text)?;
        for case in family["cases"]
            .as_array()
            .context("fixture cases must be an array")?
        {
            cases.push((
                family["family"]
                    .as_str()
                    .context("missing family")?
                    .to_owned(),
                case.clone(),
            ));
        }
    }
    cases.sort_by_key(|(_, case)| case["seq"].as_u64());
    let mut ids = BTreeSet::new();
    let mut categories = BTreeMap::<String, usize>::new();
    let mut families = BTreeMap::<String, (usize, usize)>::new();
    for (family, case) in &cases {
        let id = case["id"].as_str().context("missing fixture id")?;
        if !ids.insert(id) {
            bail!("duplicate fixture id {id}");
        }
        let counts = families.entry(family.clone()).or_default();
        counts.0 += 1;
        if let Some(entry) = ledger.get(id) {
            counts.1 += 1;
            *categories
                .entry(
                    entry["category"]
                        .as_str()
                        .context("missing ledger category")?
                        .into(),
                )
                .or_default() += 1;
        }
    }
    for id in ledger.keys() {
        if !ids.contains(id.as_str()) {
            bail!("ledger entry {id} has no baseline fixture");
        }
    }
    if cases.is_empty() {
        bail!("cannot census an empty baseline corpus");
    }
    let mut report = format!(
        "# Empty-host baseline census\n\nGenerated by `cargo xtask parity-census`. No fixtures or expected failures are changed.\n\n{} captured cases, {} ledger mismatches, {} matches. These counts describe\n`app::router(Services::default(), ServerConfig::full_host(..))`, NOT the production Process.\n`not-mounted` and `not-ported` identify absent routes/services in that host; they do not\nprove that the corresponding domain implementation is absent from production.\n`status` and `body` identify observed empty-host wire differences, also requiring production replay.\n\nStrict public check for EVERY row: `cargo test --locked --offline -p opencode-runtime --test strict_replay -- --ignored --nocapture`.\nAPI/bat reviews the HTTP contract for all domain-owned rows. No ledger entry excuses a strict failure.\n\n",
        cases.len(),
        ledger.len(),
        cases.len() - ledger.len()
    );
    writeln!(
        report,
        "\nBaseline corpus SHA-256: `{}`.\nLedger SHA-256: `{ledger_hash}`.\nCorpus hash input is sorted filename, NUL, file bytes, NUL for every JSON file.\n",
        hex::encode(corpus_hash.finalize())
    )?;
    report.push_str("## Categories\n\n| category | cases |\n| --- | --- |\n");
    for (category, count) in categories {
        writeln!(report, "| {category} | {count} |")?;
    }
    report.push_str(
        "\n## Families\n\n| family | captured | ledger mismatch |\n| --- | --- | --- |\n",
    );
    for (family, (count, debt)) in families {
        writeln!(report, "| {family} | {count} | {debt} |")?;
    }
    if let Some(filter) = filter {
        writeln!(
            report,
            "\nRoute rows below are assigned to `{filter}`. Category and family totals above describe the entire corpus."
        )?;
    }
    report.push_str("\n## Requirement to owner and public check\n\nEach fixture ID is an exact captured requirement. `match` here only means empty-host ledger conformance.\nAll production results remain unverified until the strict Process replay executes.\n\n| seq | baseline fixture requirement | operation | method and route | owner | empty-host category |\n| --- | --- | --- | --- | --- | --- |\n");
    for (_, case) in cases {
        let id = case["id"].as_str().unwrap();
        let method = case["request"]["method"]
            .as_str()
            .context("missing method")?;
        let path = case["request"]["path"].as_str().context("missing route")?;
        if filter.is_some_and(|filter| !owner(path).to_ascii_lowercase().starts_with(filter)) {
            continue;
        }
        let category = ledger
            .get(id)
            .and_then(|entry| entry["category"].as_str())
            .unwrap_or("match");
        writeln!(
            report,
            "| {} | `{}` | {} | `{method} {}` | {} | {category} |",
            case["seq"],
            id.replace('|', "\\|"),
            case["operationId"].as_str().unwrap_or("route-level"),
            path.replace('|', "\\|"),
            owner(path)
        )?;
    }
    Ok(report)
}
