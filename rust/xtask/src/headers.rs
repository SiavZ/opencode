//! Purpose: derive and enforce module dependency header trees from Rust syntax and runtime edges.
//! Scope: build tooling; --fix updates only generator-owned header blocks.
//! Uses:
//! rust/xtask/
//! |-- src/headers/syntax.rs
//! `-- src/workspace.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- char::is_lowercase
//! |-- serde::Deserialize
//! |-- syn::Expr::Lit
//! |-- syn::Lit::Str
//! |-- syn::Meta::NameValue
//! |-- syn::parse_file
//! |-- syn::visit::Visit
//! |-- toml::Value
//! |-- toml::Value::as_str
//! `-- toml::from_str
//! Used by:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/lib.rs
//! |-- src/main.rs
//! |-- tests/gates.rs
//! `-- tests/gates/headers.rs
//! Invariants: missing author fields and unresolved first-party references are errors, never invented.
//! Contract tests: xtask/tests/gates.rs

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::visit::Visit;

mod syntax;
use syntax::References;

#[derive(Default, Deserialize)]
struct Runtime {
    #[serde(default)]
    edge: Vec<RuntimeEdge>,
}
#[derive(Deserialize)]
struct RuntimeEdge {
    from: String,
    to: String,
    kind: String,
    reason: String,
}

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() && args != ["--fix"] && !(args.len() == 2 && args[0] == "--fix") {
        bail!("usage: headers [--fix [owned-subdirectory]]");
    }
    let root = crate::workspace::rust_root()?;
    let scope = args.get(1).map(|path| normalize(&root.join(path)));
    if scope.as_ref().is_some_and(|path| !path.starts_with(&root)) {
        bail!("header fix scope must stay inside rust root");
    }
    crate::workspace::finish(
        "headers",
        validate_paths(
            &root,
            args.first().is_some_and(|arg| arg == "--fix"),
            scope.as_deref(),
        )?,
    )
}

pub fn validate(root: &Path, fix: bool) -> Result<Vec<String>> {
    validate_paths(root, fix, None)
}

fn validate_paths(root: &Path, fix: bool, scope: Option<&Path>) -> Result<Vec<String>> {
    let files = crate::workspace::sources(root)?;
    let mut errors = Vec::new();
    let mut modules = BTreeMap::<String, PathBuf>::new();
    let mut identities = BTreeMap::new();
    let mut parsed = BTreeMap::new();
    let mut packages = BTreeMap::new();
    let mut roots = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let parts = relative.split('/').collect::<Vec<_>>();
        let base = if parts[0] == "xtask" { 1 } else { 2 };
        let package_dir = parts[..base].join("/");
        let manifest = root.join(&package_dir).join("Cargo.toml");
        let package = if manifest.is_file() {
            let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(manifest)?)?;
            manifest
                .get("lib")
                .and_then(|v| v.get("name"))
                .and_then(toml::Value::as_str)
                .or_else(|| {
                    manifest
                        .get("package")
                        .and_then(|v| v.get("name"))
                        .and_then(toml::Value::as_str)
                })
                .context("package name missing")?
                .replace('-', "_")
        } else if base == 1 {
            "xtask".into()
        } else {
            format!("opencode_{}", parts[1].replace('-', "_"))
        };
        packages.insert(package_dir.clone(), package.clone());
        let tail = &parts[base..];
        let is_root = tail == ["src", "lib.rs"]
            || tail == ["src", "main.rs"]
            || tail == ["build.rs"]
            || (tail.len() == 2 && matches!(tail[0], "tests" | "benches" | "examples"))
            || (tail.len() == 3 && tail[..2] == ["src", "bin"]);
        if is_root {
            let namespace = if tail == ["src", "lib.rs"] {
                package
            } else {
                format!("{package}@{relative}")
            };
            roots.push((file.clone(), namespace, Vec::<String>::new()));
        }
        let text = std::fs::read_to_string(file)?;
        match syn::parse_file(&text) {
            Ok(ast) => {
                let mut refs = References::default();
                refs.visit_file(&ast);
                parsed.insert(file.clone(), (text, refs));
            }
            Err(error) => errors.push(format!(
                "{}: Rust syntax: {error}",
                crate::workspace::display_path(root, file)
            )),
        }
    }
    let mut edges: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut externals: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    // Follow actual mod declarations from each crate/test entrypoint. A shared support
    // file can belong to multiple integration-test crates, each with its own crate:: root.
    while let Some((file, namespace, path)) = roots.pop() {
        let key = std::iter::once(namespace.clone())
            .chain(path.clone())
            .collect::<Vec<_>>()
            .join("::");
        if modules.insert(key, file.clone()).is_some() {
            continue;
        }
        identities
            .entry(file.clone())
            .or_insert_with(Vec::new)
            .push((namespace.clone(), path.clone()));
        let Some((_, refs)) = parsed.get(&file) else {
            continue;
        };
        for (scope, module) in &refs.modules {
            let directory = if path.is_empty() || file.file_name().is_some_and(|s| s == "mod.rs") {
                file.parent().context("source parent")?.to_path_buf()
            } else {
                file.with_extension("")
            };
            let directory = scope.iter().fold(directory, |dir, name| dir.join(name));
            let explicit = module.attrs.iter().find_map(|attr| {
                if !attr.path().is_ident("path") {
                    return None;
                }
                let syn::Meta::NameValue(value) = &attr.meta else {
                    return None;
                };
                let syn::Expr::Lit(lit) = &value.value else {
                    return None;
                };
                let syn::Lit::Str(value) = &lit.lit else {
                    return None;
                };
                Some(if scope.is_empty() {
                    file.parent().unwrap_or(&directory).join(value.value())
                } else {
                    directory.join(value.value())
                })
            });
            let candidates = explicit.map_or_else(
                || {
                    vec![
                        directory.join(format!("{}.rs", module.ident)),
                        directory.join(module.ident.to_string()).join("mod.rs"),
                    ]
                },
                |p| vec![p],
            );
            match candidates.into_iter().find(|p| p.is_file()) {
                Some(target) => {
                    let target = normalize(&target);
                    edges
                        .entry(file.clone())
                        .or_default()
                        .insert(target.clone());
                    roots.push((
                        target,
                        namespace.clone(),
                        path.iter()
                            .chain(scope)
                            .cloned()
                            .chain([module.ident.to_string()])
                            .collect(),
                    ));
                }
                None => errors.push(format!(
                    "{}: unresolved mod {}",
                    crate::workspace::display_path(root, &file),
                    module.ident
                )),
            }
        }
    }
    for (file, (_, refs)) in &parsed {
        let Some(contexts) = identities.get(file) else {
            errors.push(format!(
                "{}: source is not reachable from a library, binary, test, example or bench root",
                crate::workspace::display_path(root, file)
            ));
            continue;
        };
        for (namespace, path) in contexts {
            for reference in &refs.uses {
                let marker = reference
                    .iter()
                    .position(|p| p == "@")
                    .context("scope separator")?;
                let scope = &reference[..marker];
                let reference = &reference[marker + 1..];
                let Some(first) = reference.first() else {
                    continue;
                };
                let local = matches!(first.as_str(), "crate" | "super" | "self");
                let bare = std::iter::once(namespace.clone())
                    .chain(path.clone())
                    .chain(scope.to_vec())
                    .chain([first.clone()])
                    .collect::<Vec<_>>();
                let bare_local = modules.contains_key(&bare.join("::"));
                let first_party =
                    local || bare_local || packages.values().any(|package| package == first);
                if !first_party {
                    if !matches!(first.as_str(), "std" | "core" | "alloc")
                        && first.chars().next().is_some_and(char::is_lowercase)
                    {
                        externals
                            .entry(file.clone())
                            .or_default()
                            .insert(reference.join("::"));
                    }
                    continue;
                }
                let mut key = if bare_local {
                    bare
                } else if local {
                    vec![namespace.clone()]
                } else {
                    vec![first.clone()]
                };
                if local && !bare_local && first != "crate" {
                    key.extend(path.clone());
                    key.extend(scope.to_vec());
                }
                if first == "super" {
                    key.pop();
                }
                for part in reference.iter().skip(1) {
                    if part == "super" {
                        key.pop();
                    } else if part != "self" {
                        key.push(part.clone());
                    }
                }
                let target = (1..=key.len())
                    .rev()
                    .find_map(|len| modules.get(&key[..len].join("::")));
                match target {
                    Some(target) if target != file => {
                        edges
                            .entry(file.clone())
                            .or_default()
                            .insert(target.clone());
                    }
                    Some(_) => (),
                    None => errors.push(format!(
                        "{}: unresolved first-party use {}",
                        crate::workspace::display_path(root, file),
                        reference.join("::")
                    )),
                }
            }
        }
    }
    let runtime: Runtime = toml::from_str(&std::fs::read_to_string(
        root.join("contracts/runtime-edges.toml"),
    )?)?;
    for edge in runtime.edge {
        let from = root.join(edge.from.strip_prefix("rust/").unwrap_or(&edge.from));
        let to = root.join(edge.to.strip_prefix("rust/").unwrap_or(&edge.to));
        if !files.contains(&from)
            || !files.contains(&to)
            || edge.reason.trim().is_empty()
            || !matches!(
                edge.kind.as_str(),
                "runtime-registration" | "proc-macro" | "ffi" | "generated" | "dynamic-dispatch"
            )
        {
            errors.push(format!("invalid runtime edge {} -> {}", edge.from, edge.to));
            continue;
        }
        edges.entry(from).or_default().insert(to);
    }
    for (file, (text, _)) in &parsed {
        let name = crate::workspace::display_path(root, file);
        let header = text
            .lines()
            .take_while(|line| line.starts_with("//!"))
            .collect::<Vec<_>>();
        if !header
            .first()
            .is_some_and(|line| line.starts_with("//! Purpose: "))
            || ["Scope: ", "Invariants: ", "Contract tests: "]
                .iter()
                .any(|field| {
                    !header.iter().any(|line| {
                        line.strip_prefix(&format!("//! {field}"))
                            .is_some_and(|value| !value.trim().is_empty())
                    })
                })
        {
            errors.push(format!("{name}: missing author-owned header fields"));
            continue;
        }
        let Some(start) = text.find("//! Uses:\n") else {
            errors.push(format!("{name}: missing Uses block"));
            continue;
        };
        let Some(end) = text.find("//! Invariants:") else {
            continue;
        };
        if start >= end {
            errors.push(format!("{name}: header sections out of order"));
            continue;
        }
        let uses = edges
            .get(file)
            .into_iter()
            .flatten()
            .map(|p| crate::workspace::display_path(root, p))
            .collect::<BTreeSet<_>>();
        let used = edges
            .iter()
            .filter(|(_, targets)| targets.contains(file))
            .map(|(p, _)| crate::workspace::display_path(root, p))
            .collect::<BTreeSet<_>>();
        let external = externals.get(file).cloned().unwrap_or_default();
        let generated = format!(
            "//! Uses:\n{}//! External:\n{}//! Used by:\n{}",
            tree(&uses, false),
            leaves(&external),
            tree(&used, true)
        );
        if text[start..end] == generated {
            continue;
        }
        if fix && scope.is_none_or(|scope| file.starts_with(scope)) {
            std::fs::write(
                file,
                format!("{}{}{}", &text[..start], generated, &text[end..]),
            )?;
        } else {
            errors.push(format!("{name}: dependency header drift (run headers --fix after reviewing graph diagnostics)"));
        }
    }
    Ok(errors)
}

fn normalize(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut path, component| {
            if component == std::path::Component::ParentDir {
                path.pop();
            } else if component != std::path::Component::CurDir {
                path.push(component);
            }
            path
        })
}

fn leaves(entries: &BTreeSet<String>) -> String {
    if entries.is_empty() {
        return "//! `-- none\n".into();
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "//! {} {entry}\n",
                if index + 1 == entries.len() {
                    "`--"
                } else {
                    "|--"
                }
            )
        })
        .collect()
}

fn tree(paths: &BTreeSet<String>, entrypoint: bool) -> String {
    if paths.is_empty() {
        return format!(
            "//! `-- none{}\n",
            if entrypoint { ", entrypoint" } else { "" }
        );
    }
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in paths {
        let parts = path.splitn(3, '/').collect::<Vec<_>>();
        if parts.len() == 3 {
            groups
                .entry(format!("{}/{}/", parts[0], parts[1]))
                .or_default()
                .insert(parts[2].into());
        }
    }
    groups
        .into_iter()
        .map(|(root, entries)| format!("//! {root}\n{}", leaves(&entries)))
        .collect()
}
