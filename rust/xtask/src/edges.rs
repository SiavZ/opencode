//! Purpose: enforce declared first-party edges against Cargo's resolved host and wasm graphs.
//! Scope: read-only build tooling.
//! Uses:
//! rust/xtask/
//! `-- src/workspace.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- cargo_metadata::MetadataCommand::new
//! |-- serde::Deserialize
//! |-- serde_json::Value
//! |-- serde_json::to_value
//! `-- toml::from_str
//! Used by:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/lib.rs
//! |-- src/main.rs
//! `-- tests/gates.rs
//! Invariants: normal/build edges require approval; dev edges still participate in cycle checks.
//! Contract tests: xtask/tests/gates.rs

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub owner: Vec<String>,
    pub target: String,
    pub allow: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Contract {
    pub crates: BTreeMap<String, Rule>,
    pub bins: BTreeMap<String, Rule>,
    pub xtask: Rule,
}

impl Contract {
    pub fn rules(&self) -> BTreeMap<String, &Rule> {
        self.crates
            .iter()
            .map(|(name, rule)| (format!("crates/{name}"), rule))
            .chain(
                self.bins
                    .iter()
                    .map(|(name, rule)| (format!("bins/{name}"), rule)),
            )
            .chain([(String::from("xtask"), &self.xtask)])
            .collect()
    }
}

pub fn metadata(root: &Path, platform: Option<&str>) -> Result<Value> {
    let mut command = cargo_metadata::MetadataCommand::new();
    command
        .current_dir(root)
        .other_options(vec!["--locked".into(), "--offline".into()]);
    if let Some(platform) = platform {
        command.other_options(vec![
            "--locked".into(),
            "--offline".into(),
            "--filter-platform".into(),
            platform.into(),
        ]);
    }
    serde_json::to_value(
        command
            .exec()
            .context("resolve locked offline Cargo graph")?,
    )
    .context("encode Cargo metadata")
}

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        bail!("edges accepts no arguments");
    }
    let root = crate::workspace::rust_root()?;
    let contract: Contract = toml::from_str(&std::fs::read_to_string(
        root.join("contracts/crate-edges.toml"),
    )?)?;
    let host = std::process::Command::new("rustc").arg("-vV").output()?;
    if !host.status.success() {
        bail!("rustc -vV failed");
    }
    let text = String::from_utf8(host.stdout)?;
    let triple = text
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("rustc omitted host triple")?;
    let mut errors = Vec::new();
    for platform in [triple, "wasm32-unknown-unknown"] {
        let graph = metadata(&root, Some(platform))?;
        errors.extend(
            validate(&root, &contract, &graph)?
                .into_iter()
                .map(|error| format!("{platform}: {error}")),
        );
    }
    crate::workspace::finish("edges", errors)
}

pub fn validate(root: &Path, contract: &Contract, graph: &Value) -> Result<Vec<String>> {
    let rules = contract.rules();
    let members = graph["workspace_members"]
        .as_array()
        .context("workspace_members missing")?;
    let packages = graph["packages"].as_array().context("packages missing")?;
    let mut errors = Vec::new();
    let mut names = BTreeMap::new();
    for package in packages.iter().filter(|p| members.contains(&p["id"])) {
        let manifest = Path::new(
            package["manifest_path"]
                .as_str()
                .context("manifest path missing")?,
        );
        let key = manifest
            .parent()
            .context("manifest parent missing")?
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if !rules.contains_key(&key) {
            errors.push(format!("undeclared workspace member {key}"));
        }
        names.insert(
            package["id"]
                .as_str()
                .context("package id missing")?
                .to_owned(),
            key,
        );
    }
    for (name, rule) in &rules {
        if !names.values().any(|key| key == name) {
            errors.push(format!("stale declaration {name}: no workspace member"));
        }
        if rule.owner.is_empty()
            || !matches!(
                rule.target.as_str(),
                "native" | "portable" | "wasm" | "worker"
            )
        {
            errors.push(format!("{name}: invalid owner/target metadata"));
        }
        for allowed in &rule.allow {
            if !rules.contains_key(&format!("crates/{allowed}")) {
                errors.push(format!("{name}: unknown allowed crate {allowed}"));
            }
        }
    }
    let mut combined: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut runtime: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in graph["resolve"]["nodes"]
        .as_array()
        .context("resolved graph missing")?
    {
        let Some(from) = node["id"].as_str().and_then(|id| names.get(id)) else {
            continue;
        };
        for dep in node["deps"]
            .as_array()
            .context("resolved dependencies missing")?
        {
            let Some(to) = dep["pkg"].as_str().and_then(|id| names.get(id)) else {
                continue;
            };
            combined.entry(from.clone()).or_default().insert(to.clone());
            let kinds = dep["dep_kinds"]
                .as_array()
                .context("dependency kinds missing")?;
            if !kinds.iter().any(|kind| kind["kind"] != "dev") {
                continue;
            }
            runtime.entry(from.clone()).or_default().insert(to.clone());
            if let Some(rule) = rules.get(from)
                && !rule
                    .allow
                    .iter()
                    .any(|allowed| to == &format!("crates/{allowed}"))
            {
                errors.push(format!("unapproved normal/build edge {from} -> {to}"));
            }
        }
    }
    for start in names.values() {
        let mut pending = combined
            .get(start)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(next) = pending.pop() {
            if next == *start {
                errors.push(format!(
                    "combined normal/build/dev dependency cycle through {start}"
                ));
                break;
            }
            if visited.insert(next.clone()) {
                pending.extend(combined.get(&next).into_iter().flatten().cloned());
            }
        }
        if rules.get(start).is_none_or(|rule| rule.target == "native") {
            continue;
        }
        let mut pending = vec![start.clone()];
        let mut visited = BTreeSet::new();
        while let Some(next) = pending.pop() {
            if !visited.insert(next.clone()) {
                continue;
            }
            if rules.get(&next).is_some_and(|rule| rule.target == "native") {
                errors.push(format!(
                    "portable/wasm closure {start} resolves native-only {next}"
                ));
            }
            pending.extend(runtime.get(&next).into_iter().flatten().cloned());
        }
    }
    errors.sort();
    errors.dedup();
    Ok(errors)
}
