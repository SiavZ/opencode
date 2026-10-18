//! Purpose: stage policy and the resource graph model that mirrors `sst.config.ts` + `infra/*.ts`.
//! Scope: pure data, no I/O. Emitters (terraform.rs) and checks (inventory.rs) consume it.
//! Uses:
//! `-- none
//! External:
//! |-- indexmap::IndexMap
//! `-- serde_json::{json, Value}
//! Used by:
//! rust/xtask/
//! |-- src/cloud/infra/app.rs
//! |-- src/cloud/infra/console.rs
//! |-- src/cloud/infra/enterprise.rs
//! |-- src/cloud/infra/mod.rs
//! |-- src/cloud/infra/monitoring.rs
//! |-- src/cloud/infra/stage.rs
//! |-- src/cloud/infra/stats.rs
//! |-- src/cloud/inventory.rs
//! |-- src/cloud/mod.rs
//! `-- src/cloud/terraform.rs
//! Invariants: resource order is source order, references are symbolic (`${ref:Name.attr}`),
//! secret values never appear, only secret names. Stage rules match sst.config.ts:3-46 and
//! infra/stage.ts:1-40 exactly.
//! Contract tests: xtask/tests/cloud_plan.rs

use indexmap::IndexMap;
use serde_json::Value;

/// One SST stage plus the `$dev` flag (`sst dev` mode, infra/console.ts:286-291).
#[derive(Debug, Clone)]
pub struct Stage {
    pub name: String,
    pub sst_dev: bool,
}

impl Stage {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            sst_dev: false,
        }
    }

    pub fn is_production(&self) -> bool {
        self.name == "production"
    }

    /// infra/stage.ts:1-5
    pub fn domain(&self) -> String {
        match self.name.as_str() {
            "production" => "opencode.ai".into(),
            "dev" => "dev.opencode.ai".into(),
            other => format!("{other}.dev.opencode.ai"),
        }
    }

    /// infra/stage.ts:36-40
    pub fn short_domain(&self) -> String {
        match self.name.as_str() {
            "production" => "opncd.ai".into(),
            "dev" => "dev.opncd.ai".into(),
            other => format!("{other}.dev.opncd.ai"),
        }
    }

    /// infra/stage.ts:8
    pub fn aws_stage(&self) -> &'static str {
        if self.is_production() {
            "production"
        } else {
            "dev"
        }
    }

    /// infra/stage.ts:9, only `production` and `dev` deploy the AWS stats stack.
    pub fn deploy_aws(&self) -> bool {
        self.name == self.aws_stage()
    }

    /// sst.config.ts:36
    pub fn monitoring(&self) -> bool {
        self.is_production() || self.name == "vimtor"
    }

    /// sst.config.ts:7
    pub fn removal(&self) -> &'static str {
        if self.is_production() {
            "retain"
        } else {
            "remove"
        }
    }

    /// sst.config.ts:8
    pub fn protect(&self) -> bool {
        self.is_production()
    }

    /// sst.config.ts:14-18, profile is unset under GitHub Actions.
    pub fn aws_profile(&self, github_actions: bool) -> Option<&'static str> {
        if github_actions {
            return None;
        }
        Some(if self.is_production() {
            "opencode-production"
        } else {
            "opencode-dev"
        })
    }
}

/// How a node participates in deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Provisioned resource or SST component.
    Managed,
    /// Read-only lookup (`get*Output`).
    Data,
    /// `sst.Secret`: a named input supplied out of band.
    Secret,
    /// `sst.Linkable`: link-only property bag, nothing provisioned.
    Linkable,
    /// `sst.x.DevCommand`: only runs under `sst dev`, never deployed.
    DevCommand,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub name: String,
    pub kind: &'static str,
    pub mode: Mode,
    /// `file:line-range` in the baseline.
    pub source: &'static str,
    pub props: Value,
    pub links: Vec<String>,
}

impl Resource {
    pub fn new(mode: Mode, kind: &'static str, name: &str, source: &'static str) -> Self {
        Self {
            name: name.to_string(),
            kind,
            mode,
            source,
            props: Value::Null,
            links: Vec::new(),
        }
    }

    pub fn props(mut self, props: Value) -> Self {
        self.props = props;
        self
    }

    pub fn links<I: IntoIterator<Item = S>, S: Into<String>>(mut self, links: I) -> Self {
        self.links = links.into_iter().map(Into::into).collect();
        self
    }
}

/// Symbolic reference to another node's attribute, resolved by emitters.
pub fn reference(name: &str, attr: &str) -> Value {
    Value::String(format!("${{ref:{name}.{attr}}}"))
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub stage: Stage,
    pub resources: Vec<Resource>,
    pub outputs: IndexMap<String, Value>,
}

impl Plan {
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            resources: Vec::new(),
            outputs: IndexMap::new(),
        }
    }

    pub fn add(&mut self, resource: Resource) -> String {
        let name = resource.name.clone();
        self.resources.push(resource);
        name
    }

    pub fn get(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }
}
