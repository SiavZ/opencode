//! Purpose: platform target matrix (decision D13) model and per-row execution: decide whether a
//! row is runnable here, run the artifact smoke (`<artifact> --version`) on the host or inside a
//! `docker --platform` container, and report a typed per-row outcome.
//! Scope: Phase 15 platform runner skeleton. Full per-row acceptance reuses rust/tests/acceptance.
//! Uses:
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- toml::from_str
//! Used by:
//! rust/tests/
//! |-- platform/src/main.rs
//! `-- platform/tests/matrix.rs
//! Invariants: a `none` runner, a missing docker daemon, a host triple mismatch or a missing
//! artifact is always `blocked`, never `pass`; only an executed smoke can pass.
//! Contract tests: platform/tests/matrix.rs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct Matrix {
    #[serde(rename = "target")]
    pub targets: Vec<Row>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub triple: String,
    pub runner: Runner,
    #[serde(default)]
    pub host_triple: Option<String>,
    #[serde(default)]
    pub docker_platform: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runner {
    Host,
    Docker,
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub id: String,
    pub triple: String,
    pub runner: Runner,
    pub decision: String,
    pub detail: String,
}

/// What the runner can observe about the machine, injectable for tests.
#[derive(Debug, Clone)]
pub struct Host {
    pub triple: String,
    pub docker: bool,
}

impl Host {
    pub fn detect() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "apple-darwin",
            "linux" => "unknown-linux-gnu",
            "windows" => "pc-windows-msvc",
            other => other,
        };
        let docker = Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|o| o.status.success());
        Self {
            triple: format!("{}-{os}", std::env::consts::ARCH),
            docker,
        }
    }
}

impl Matrix {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }
}

impl Row {
    /// Env var holding the release artifact path for this row.
    pub fn artifact_var(&self) -> String {
        format!(
            "OPENCODE_ARTIFACT_{}",
            self.id.to_uppercase().replace('-', "_")
        )
    }

    /// `Err(reason)` when the row cannot execute on this machine.
    pub fn runnable(&self, host: &Host) -> std::result::Result<(), String> {
        match self.runner {
            Runner::None => Err("blocked: runner unavailable (D13)".into()),
            Runner::Host if self.host_triple.as_deref() != Some(host.triple.as_str()) => {
                Err(format!(
                    "blocked: host is {}, row needs {}",
                    host.triple,
                    self.host_triple.as_deref().unwrap_or("?")
                ))
            }
            Runner::Docker if !host.docker => Err("blocked: docker daemon unavailable".into()),
            Runner::Docker if self.docker_platform.is_none() || self.image.is_none() => {
                Err("blocked: docker row missing docker_platform/image".into())
            }
            _ => Ok(()),
        }
    }

    pub fn run(&self, host: &Host, artifact: Option<PathBuf>) -> Outcome {
        let (decision, detail) = match (self.runnable(host), artifact) {
            (Err(reason), _) => (reason, String::new()),
            (Ok(()), None) => (
                format!(
                    "blocked: artifact not built ({} unset)",
                    self.artifact_var()
                ),
                String::new(),
            ),
            (Ok(()), Some(path)) => match self.smoke(&path) {
                Ok(out) => ("pass (smoke only, pending acceptance)".into(), out),
                Err(e) => ("fail".into(), format!("{e:#}")),
            },
        };
        Outcome {
            id: self.id.clone(),
            triple: self.triple.clone(),
            runner: self.runner,
            decision,
            detail,
        }
    }

    fn smoke(&self, artifact: &Path) -> Result<String> {
        let artifact = artifact
            .canonicalize()
            .with_context(|| format!("artifact {}", artifact.display()))?;
        let mut cmd = match self.runner {
            Runner::Docker => {
                let mut c = Command::new("docker");
                c.args(["run", "--rm", "--network", "none", "--platform"])
                    .arg(self.docker_platform.as_deref().unwrap_or_default())
                    .arg("-v")
                    .arg(format!("{}:/opt/opencode:ro", artifact.display()))
                    .arg(self.image.as_deref().unwrap_or_default())
                    .args(["/opt/opencode", "--version"]);
                c
            }
            _ => {
                let mut c = Command::new(&artifact);
                c.arg("--version");
                c
            }
        };
        let out = cmd.output()?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        anyhow::ensure!(
            out.status.success() && !stdout.is_empty(),
            "--version exit {:?} stdout {stdout:?} stderr {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(format!("--version -> {stdout}"))
    }
}
