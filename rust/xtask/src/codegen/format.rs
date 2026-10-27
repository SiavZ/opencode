//! Purpose: normalize generated Rust formatting and graph headers before byte comparison.
//! Scope: isolated copy of repository Rust sources, never a repository edit.
//! Uses:
//! |-- rust/xtask/src/codegen/mod.rs
//! |-- rust/xtask/src/headers.rs
//! `-- rust/xtask/src/workspace.rs
//! External:
//! `-- anyhow
//! Used by:
//! `-- rust/xtask/src/codegen/mod.rs
//! Invariants: only generated files return to the caller; unrelated author headers stay untouched.
//! Contract tests: xtask/tests/codegen.rs
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn format(files: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let root = std::env::temp_dir().join(format!(
        "opencode-codegen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&root)?;
    let result = (|| {
        let source = crate::workspace::rust_root()?;
        let generated = root.join("crates/client/src/generated");
        let mut manifests = BTreeSet::new();
        for path in crate::workspace::sources(&source)? {
            let relative = path.strip_prefix(&source)?;
            if relative.starts_with("crates/client/src/generated") {
                continue;
            }
            let destination = root.join(relative);
            std::fs::create_dir_all(destination.parent().expect("source parent"))?;
            std::fs::copy(&path, &destination)?;
            let components = relative.components().collect::<Vec<_>>();
            manifests.insert(
                components
                    .iter()
                    .take(if components[0].as_os_str() == "xtask" {
                        1
                    } else {
                        2
                    })
                    .collect::<std::path::PathBuf>()
                    .join("Cargo.toml"),
            );
        }
        for path in manifests
            .into_iter()
            .chain([std::path::PathBuf::from("contracts/runtime-edges.toml")])
        {
            if !source.join(&path).is_file() {
                continue;
            }
            std::fs::create_dir_all(root.join(&path).parent().expect("manifest parent"))?;
            std::fs::copy(source.join(&path), root.join(path))?;
        }
        for (path, content) in &files {
            let path = generated.join(path);
            std::fs::create_dir_all(path.parent().expect("generated parent"))?;
            std::fs::write(path, content)?;
        }
        let output = std::process::Command::new("rustfmt")
            .args(["--edition", "2024"])
            .arg(generated.join("mod.rs"))
            .output()?;
        if !output.status.success() {
            bail!(
                "generated Rust formatting failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let errors = crate::headers::validate(&root, true)?
            .into_iter()
            .filter(|error| error.starts_with("rust/crates/client/src/generated/"))
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            bail!("generated headers failed: {}", errors.join("\n"));
        }
        files
            .keys()
            .map(|path| Ok((path.clone(), std::fs::read_to_string(generated.join(path))?)))
            .collect()
    })();
    std::fs::remove_dir_all(&root)?;
    result
}
