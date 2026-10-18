//! Purpose: server root detection: `NearestRoot`, `StrictNearestRoot`, and the bespoke root
//! functions for gopls, rust-analyzer, jdtls (Maven module chain), kotlin-ls and nixd.
//! Ports packages/opencode/src/lsp/server.ts:32-78 plus the per-server `root` bodies.
//! Scope: filesystem reads only (existence checks, Cargo.toml / pom.xml reads).
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/lsp/text.rs
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/inventory.rs
//! `-- integration-adapters/tests/lsp_roots.rs
//! Invariants: walks from dirname(file) up to and including `directory`, targets are checked
//! per directory in declaration order, targets are literal names (baseline `fs.stat(join(dir,t))`,
//! so `*.tf` style patterns only match a file literally named that way).
//! Contract tests: integration-adapters/tests/lsp_roots.rs

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::text::normalize;

/// Instance context used by root functions (`ctx.directory`, `ctx.worktree`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootContext {
    pub directory: PathBuf,
    pub worktree: PathBuf,
}

/// Declarative root strategy for one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootRule {
    /// `ctx.directory` always.
    Directory,
    /// Nearest marker dir, else `ctx.directory`. Excluded markers return None.
    Nearest {
        include: &'static [&'static str],
        exclude: &'static [&'static str],
    },
    /// Nearest marker dir only (deno).
    Strict {
        include: &'static [&'static str],
    },
    Gopls,
    Rust,
    Jdtls,
    Kotlin,
    Nixd,
}

/// First match of `Filesystem.up({targets, start, stop})`.
pub fn up_first(targets: &[&str], start: &Path, stop: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        for t in targets {
            let candidate = current.join(t);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        if current == stop {
            return None;
        }
        let parent = current.parent()?.to_path_buf();
        current = parent;
    }
}

fn dirname(p: &Path) -> PathBuf {
    p.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| p.to_path_buf())
}

fn nearest(
    include: &[&str],
    exclude: &[&str],
    file: &Path,
    ctx: &RootContext,
    strict: bool,
) -> Option<PathBuf> {
    let start = dirname(file);
    if !exclude.is_empty() && up_first(exclude, &start, &ctx.directory).is_some() {
        return None;
    }
    match up_first(include, &start, &ctx.directory) {
        Some(hit) => Some(dirname(&hit)),
        None if strict => None,
        None => Some(ctx.directory.clone()),
    }
}

impl RootRule {
    pub fn resolve(&self, file: &Path, ctx: &RootContext) -> Option<PathBuf> {
        let file = normalize(file);
        let file = file.as_path();
        match self {
            Self::Directory => Some(ctx.directory.clone()),
            Self::Nearest { include, exclude } => nearest(include, exclude, file, ctx, false),
            Self::Strict { include } => nearest(include, &[], file, ctx, true),
            Self::Gopls => nearest(&["go.work"], &[], file, ctx, false)
                .or_else(|| nearest(&["go.mod", "go.sum"], &[], file, ctx, false)),
            Self::Rust => rust_root(file, ctx),
            Self::Jdtls => jdtls_root(file, ctx),
            Self::Kotlin => kotlin_root(file, ctx),
            Self::Nixd => {
                let flake = nearest(&["flake.nix"], &[], file, ctx, false);
                if let Some(root) = flake.filter(|r| *r != ctx.directory) {
                    return Some(root);
                }
                if ctx.worktree != ctx.directory {
                    return Some(ctx.worktree.clone());
                }
                Some(ctx.directory.clone())
            }
        }
    }
}

/// server.ts:892-920: crate root, then the nearest ancestor `Cargo.toml` with `[workspace]`
/// while staying under `ctx.worktree`.
fn rust_root(file: &Path, ctx: &RootContext) -> Option<PathBuf> {
    let crate_root = nearest(&["Cargo.toml", "Cargo.lock"], &[], file, ctx, false)?;
    let mut current = crate_root.clone();
    while let Some(parent) = current.parent().map(Path::to_path_buf) {
        if let Ok(text) = std::fs::read_to_string(current.join("Cargo.toml"))
            && text.contains("[workspace]")
        {
            return Some(current);
        }
        if parent == current {
            break;
        }
        current = parent;
        if !current
            .to_string_lossy()
            .starts_with(&*ctx.worktree.to_string_lossy())
        {
            break;
        }
    }
    Some(crate_root)
}

/// server.ts:1149-1186.
fn jdtls_root(file: &Path, ctx: &RootContext) -> Option<PathBuf> {
    let settings = ["settings.gradle", "settings.gradle.kts"];
    let start = dirname(file);
    let wrapper = if up_first(&settings, &start, &ctx.directory).is_some() {
        None
    } else {
        nearest(&["gradlew", "gradlew.bat"], &[], file, ctx, true)
    };
    if let Some(root) = wrapper.or_else(|| nearest(&settings, &[], file, ctx, true)) {
        return Some(root);
    }
    if let Some(root) = nearest(&["build.gradle", "build.gradle.kts"], &[], file, ctx, true) {
        return Some(root);
    }
    let poms = find_up_all("pom.xml", &start, &ctx.directory);
    if let Some(first) = poms.first() {
        let mut root = dirname(first);
        for pom in &poms[1..] {
            let parent = dirname(pom);
            let rel = super::text::relative(&parent, &root);
            let content = std::fs::read_to_string(pom).ok();
            if content.is_some_and(|c| is_module_of(&c, &rel)) {
                root = parent;
                continue;
            }
            break;
        }
        return Some(root);
    }
    nearest(&[".project", ".classpath"], &[], file, ctx, true)
}

/// `Filesystem.findUp(target, start, stop)`: every hit from start upward, stop inclusive.
fn find_up_all(target: &str, start: &Path, stop: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(target);
        if candidate.exists() {
            out.push(candidate);
        }
        if current == stop {
            break;
        }
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            break;
        };
        current = parent;
    }
    out
}

static MODULES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<modules>(.*?)</modules>").expect("regex"));
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("regex"));
static MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<module>\s*([^<]+?)\s*</module>").expect("regex"));

/// server.ts:1133-1145 `isModuleOf`.
pub fn is_module_of(pom: &str, module_path: &str) -> bool {
    let normalized = module_path.replace('\\', "/");
    let normalized = normalized.strip_suffix('/').unwrap_or(&normalized);
    if normalized.is_empty() {
        return false;
    }
    MODULES.find_iter(pom).any(|block| {
        let stripped = COMMENT.replace_all(block.as_str(), "");
        MODULE.captures_iter(&stripped).any(|m| {
            let decl = m[1].replace('\\', "/");
            let decl = decl.strip_prefix("./").unwrap_or(&decl);
            decl.strip_suffix('/').unwrap_or(decl) == normalized
        })
    })
}

/// server.ts:1276-1288. NearestRoot never returns None, so step 1 always wins (baseline quirk).
fn kotlin_root(file: &Path, ctx: &RootContext) -> Option<PathBuf> {
    nearest(
        &["settings.gradle.kts", "settings.gradle"],
        &[],
        file,
        ctx,
        false,
    )
}
