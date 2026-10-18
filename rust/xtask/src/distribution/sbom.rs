//! Purpose: CycloneDX 1.5 JSON SBOM for one binary package, generated in Rust from
//! `cargo metadata` (resolve graph, licenses, sources) and Cargo.lock (registry SHA-256 checksums).
//! Scope: spawns `cargo metadata --locked`, reads Cargo.lock, pure transformation otherwise.
//! Uses:
//! rust/xtask/
//! `-- src/distribution/mod.rs
//! External:
//! |-- serde_json::{json, Value}
//! |-- sha2::{Digest, Sha256}
//! `-- toml::Table
//! Used by:
//! rust/xtask/
//! |-- src/distribution/build.rs
//! |-- src/distribution/mod.rs
//! `-- src/distribution/tests.rs
//! Invariants: no timestamps, components sorted by bom-ref, serialNumber derived from content, so
//! the SBOM is reproducible. Only normal and build edges reachable from the package are included,
//! dev-dependencies never are. Build-only crates get scope `excluded`.
//! Contract tests: xtask/src/distribution/tests.rs

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::DistError;

pub fn generate(
    root: &Path,
    package: &str,
    triple: Option<&str>,
    artifact: Option<&Path>,
) -> Result<Value> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"]);
    if let Some(triple) = triple {
        cmd.args(["--filter-platform", triple]);
    }
    let output = cmd.output().context("running cargo metadata")?;
    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let artifact = artifact
        .map(|p| std::fs::read(p).with_context(|| format!("reading {}", p.display())))
        .transpose()?;
    Ok(from_metadata(
        &metadata,
        &lock,
        package,
        artifact.as_deref(),
    )?)
}

/// Pure SBOM construction from captured inputs, unit-testable with fixtures.
pub fn from_metadata(
    metadata: &Value,
    lock: &str,
    package: &str,
    artifact: Option<&[u8]>,
) -> Result<Value, DistError> {
    let packages: BTreeMap<&str, &Value> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let root_id = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|id| packages.get(id).is_some_and(|p| p["name"] == package))
        .ok_or_else(|| DistError::PackageMissing(package.to_string()))?;
    let nodes: BTreeMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let checksums = lock_checksums(lock);

    // Walk normal/build edges; remember whether a crate is reachable through a normal-only path.
    let mut runtime = BTreeSet::new();
    let mut build_only = BTreeSet::new();
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut stack = vec![(root_id, true)];
    while let Some((id, is_runtime)) = stack.pop() {
        let seen = if is_runtime {
            !runtime.insert(id)
        } else {
            runtime.contains(id) || !build_only.insert(id)
        };
        if seen {
            continue;
        }
        for dep in nodes
            .get(id)
            .and_then(|n| n["deps"].as_array())
            .into_iter()
            .flatten()
        {
            let kinds: Vec<&str> = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|k| k["kind"].as_str().unwrap_or("normal"))
                .collect();
            if kinds.iter().all(|k| *k == "dev") {
                continue;
            }
            let Some(dep_id) = dep["pkg"].as_str() else {
                continue;
            };
            edges.entry(id).or_default().insert(dep_id);
            stack.push((dep_id, is_runtime && kinds.contains(&"normal")));
        }
    }
    build_only.retain(|id| !runtime.contains(id));

    let root_pkg = packages[root_id];
    let mut root = component(root_pkg, &checksums, None);
    if let Some(bytes) = artifact {
        root["hashes"] =
            json!([{ "alg": "SHA-256", "content": hex::encode(Sha256::digest(bytes)) }]);
    }
    let mut components: Vec<Value> = runtime
        .iter()
        .chain(build_only.iter())
        .filter(|id| **id != root_id)
        .filter_map(|id| {
            packages
                .get(id)
                .map(|p| component(p, &checksums, build_only.contains(id).then_some("excluded")))
        })
        .collect();
    components.sort_by(|a, b| a["bom-ref"].as_str().cmp(&b["bom-ref"].as_str()));
    let dependencies: Vec<Value> = runtime
        .iter()
        .chain(build_only.iter())
        .filter_map(|id| packages.get(id))
        .map(|p| {
            let mut on: Vec<String> = edges
                .get(p["id"].as_str().unwrap_or_default())
                .into_iter()
                .flatten()
                .filter_map(|d| packages.get(d))
                .map(|d| purl(d))
                .collect();
            on.sort();
            on.dedup();
            (purl(p), json!({ "ref": purl(p), "dependsOn": on }))
        })
        .collect::<BTreeMap<String, Value>>()
        .into_values()
        .collect();
    let digest = Sha256::digest(serde_json::to_vec(&json!([root, components])).unwrap_or_default());
    Ok(json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": format!("urn:uuid:{}", uuid_from(&digest)),
        "version": 1,
        "metadata": {
            "tools": { "components": [{ "type": "application", "name": "xtask distribution sbom", "version": env!("CARGO_PKG_VERSION") }] },
            "component": root,
        },
        "components": components,
        "dependencies": dependencies,
    }))
}

pub fn render(doc: &Value) -> String {
    let mut text = serde_json::to_string_pretty(doc).expect("sbom serializes");
    text.push('\n');
    text
}

fn component(
    pkg: &Value,
    checksums: &BTreeMap<(String, String), String>,
    scope: Option<&str>,
) -> Value {
    let name = pkg["name"].as_str().unwrap_or_default();
    let version = pkg["version"].as_str().unwrap_or_default();
    let mut c = json!({
        "type": if pkg["targets"].as_array().into_iter().flatten().any(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin"))) { "application" } else { "library" },
        "bom-ref": purl(pkg),
        "name": name,
        "version": version,
        "purl": purl(pkg),
    });
    if let Some(license) = pkg["license"].as_str() {
        c["licenses"] = json!([{ "expression": license }]);
    }
    if let Some(sum) = checksums.get(&(name.to_string(), version.to_string())) {
        c["hashes"] = json!([{ "alg": "SHA-256", "content": sum }]);
    }
    if let Some(scope) = scope {
        c["scope"] = json!(scope);
    }
    let source = pkg["source"].as_str();
    c["properties"] =
        json!([{ "name": "cargo:source", "value": source.unwrap_or("path (workspace)") }]);
    c
}

/// `pkg:cargo/<name>@<version>`, with a `vcs_url` qualifier for git sources. Workspace path
/// crates have no registry identity; they keep the plain cargo purl.
pub fn purl(pkg: &Value) -> String {
    let base = format!(
        "pkg:cargo/{}@{}",
        pkg["name"].as_str().unwrap_or_default(),
        pkg["version"].as_str().unwrap_or_default()
    );
    match pkg["source"].as_str() {
        Some(src) if src.starts_with("git+") => format!(
            "{base}?vcs_url={}",
            src.replace(':', "%3A").replace('#', "%23")
        ),
        _ => base,
    }
}

/// (name, version) -> registry SHA-256 from Cargo.lock `[[package]] checksum`.
pub fn lock_checksums(lock: &str) -> BTreeMap<(String, String), String> {
    let Ok(table) = lock.parse::<toml::Table>() else {
        return BTreeMap::new();
    };
    table
        .get("package")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                (
                    p.get("name")?.as_str()?.to_string(),
                    p.get("version")?.as_str()?.to_string(),
                ),
                p.get("checksum")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// RFC 4122 v4-shaped UUID from the first 16 digest bytes (content-derived, reproducible).
fn uuid_from(digest: &[u8]) -> String {
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}
