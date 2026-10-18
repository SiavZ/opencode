//! Purpose: load `contracts/phase-13/cloud-inventory.toml` and diff it against generated plans so
//! a resource added to, or dropped from, the generator fails loudly.
//! Scope: TOML parsing plus pure comparison.
//! Uses:
//! rust/xtask/
//! `-- src/cloud/model.rs
//! External:
//! |-- serde::Deserialize
//! `-- toml
//! Used by:
//! rust/xtask/
//! |-- src/cloud/mod.rs
//! `-- src/cloud/tests.rs
//! Invariants: every inventory row carries a baseline `source` locator and a `when` condition,
//! diff compares (name, kind, mode) sets exactly.
//! Contract tests: xtask/src/cloud/tests.rs

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::cloud::model::{Mode, Plan, Stage};

#[derive(Debug, Deserialize)]
pub struct Inventory {
    pub resource: Vec<Row>,
}

#[derive(Debug, Deserialize)]
pub struct Row {
    pub name: String,
    pub kind: String,
    pub mode: String,
    pub source: String,
    /// One of: always, production, non_production, deploy_aws, monitoring, sst_dev, dev_stage.
    pub when: String,
    pub provider: String,
}

pub fn parse(text: &str) -> Result<Inventory, toml::de::Error> {
    toml::from_str(text)
}

pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Managed => "managed",
        Mode::Data => "data",
        Mode::Secret => "secret",
        Mode::Linkable => "linkable",
        Mode::DevCommand => "dev_command",
    }
}

fn applies(when: &str, stage: &Stage) -> Option<bool> {
    Some(match when {
        "always" => true,
        "production" => stage.is_production(),
        "non_production" => !stage.is_production(),
        "deploy_aws" => stage.deploy_aws(),
        "monitoring" => stage.monitoring(),
        "sst_dev" => stage.sst_dev,
        // deploy_aws stages other than production, i.e. exactly `dev`.
        "dev_stage" => stage.deploy_aws() && !stage.is_production(),
        _ => return None,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub missing: Vec<String>,
    pub extra: Vec<String>,
    pub invalid: Vec<String>,
}

impl Diff {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.invalid.is_empty()
    }
}

/// `missing`: in the inventory for this stage but not generated. `extra`: generated but not inventoried.
pub fn diff(inventory: &Inventory, plan: &Plan) -> Diff {
    let mut out = Diff::default();
    let mut expected = BTreeSet::new();
    for row in &inventory.resource {
        match applies(&row.when, &plan.stage) {
            Some(true) => {
                expected.insert(format!("{} {} {}", row.name, row.kind, row.mode));
            }
            Some(false) => {}
            None => out
                .invalid
                .push(format!("{}: unknown when `{}`", row.name, row.when)),
        }
        if row.source.is_empty() || row.provider.is_empty() {
            out.invalid
                .push(format!("{}: empty source or provider", row.name));
        }
    }
    let actual: BTreeSet<String> = plan
        .resources
        .iter()
        .map(|r| format!("{} {} {}", r.name, r.kind, mode_name(r.mode)))
        .collect();
    out.missing = expected.difference(&actual).cloned().collect();
    out.extra = actual.difference(&expected).cloned().collect();
    let names: Vec<&String> = plan.resources.iter().map(|r| &r.name).collect();
    let unique: BTreeSet<&String> = names.iter().copied().collect();
    if unique.len() != names.len() {
        out.invalid.push("duplicate resource names in plan".into());
    }
    out
}
