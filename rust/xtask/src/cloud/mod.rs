//! Purpose: `cargo xtask cloud <plan|diff|golden>`: generate declarative infrastructure from the
//! Rust port of sst.config.ts + infra/*.ts, check it against the phase-13 inventory and goldens.
//! Scope: build-time tooling only. Never deploys, never calls a cloud API, needs no credentials.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/mod.rs
//! |-- src/cloud/inventory.rs
//! |-- src/cloud/model.rs
//! |-- src/cloud/terraform.rs
//! `-- src/cloud/tests.rs
//! External:
//! |-- anyhow::{bail, Context, Result}
//! `-- serde_json
//! Used by:
//! rust/xtask/
//! `-- src/main.rs
//! Invariants: output is byte-deterministic for a stage, contains no secret values, golden
//! stages are production, dev and the preview stage `pr-123`.
//! Contract tests: xtask/src/cloud/tests.rs

pub mod infra;
pub mod inventory;
pub mod model;
pub mod terraform;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use model::Stage;

pub const GOLDEN_STAGES: [&str; 3] = ["production", "dev", "pr-123"];
/// Stages with special-cased behavior in the baseline, checked by `diff`.
pub const DIFF_STAGES: [&str; 6] = ["production", "dev", "pr-123", "vimtor", "adam", "thdxr"];

const USAGE: &str = "cargo xtask cloud plan --stage <stage> [--sst-dev] [--out <dir>]\n\
cargo xtask cloud diff\n\
cargo xtask cloud golden [--check|--write]";

pub fn rust_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn render(stage: Stage) -> String {
    let plan = infra::build(stage);
    let mut text = serde_json::to_string_pretty(&terraform::emit(&plan)).unwrap_or_default();
    text.push('\n');
    text
}

pub fn load_inventory() -> Result<inventory::Inventory> {
    let path = rust_root().join("contracts/phase-13/cloud-inventory.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(inventory::parse(&text)?)
}

pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("plan") => plan(&args[1..]),
        Some("diff") => diff(),
        Some("golden") => golden(args.get(1).map(String::as_str) == Some("--write")),
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

fn plan(args: &[String]) -> Result<()> {
    let mut stage = None;
    let mut out = None;
    let mut sst_dev = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--stage" => stage = iter.next().cloned(),
            "--out" => out = iter.next().map(PathBuf::from),
            "--sst-dev" => sst_dev = true,
            other => bail!("unexpected argument {other}\n{USAGE}"),
        }
    }
    let Some(name) = stage else {
        bail!("--stage is required\n{USAGE}")
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid stage name {name:?}");
    }
    let text = render(Stage {
        name: name.clone(),
        sst_dev,
    });
    match out {
        Some(dir) => {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("main.tf.json"), text)?;
            println!("wrote {}", dir.join("main.tf.json").display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn diff() -> Result<()> {
    let inventory = load_inventory()?;
    let mut failed = false;
    for name in DIFF_STAGES {
        for sst_dev in [false, true] {
            let plan = infra::build(Stage {
                name: name.into(),
                sst_dev,
            });
            let result = inventory::diff(&inventory, &plan);
            if !result.is_clean() {
                failed = true;
                println!("stage {name} (sst_dev={sst_dev}): {result:#?}");
            }
        }
    }
    if failed {
        bail!("cloud inventory diff failed");
    }
    println!(
        "cloud inventory matches generated plans for {} stages",
        DIFF_STAGES.len()
    );
    Ok(())
}

fn golden(write: bool) -> Result<()> {
    let dir = rust_root().join("contracts/phase-13/cloud-golden");
    let mut stale = Vec::new();
    for name in GOLDEN_STAGES {
        let path = dir.join(format!("{name}.tf.json"));
        let text = render(Stage::new(name));
        if write {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, &text)?;
            continue;
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(path.display().to_string());
        }
    }
    if !stale.is_empty() {
        bail!("stale cloud goldens (run `cargo xtask cloud golden --write`): {stale:?}");
    }
    println!(
        "cloud goldens {}",
        if write { "written" } else { "up to date" }
    );
    Ok(())
}
