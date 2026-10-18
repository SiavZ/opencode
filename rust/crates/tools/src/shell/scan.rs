//! Purpose: legacy shell permission scan (packages/opencode/src/tool/shell.ts:84-260,374-410): parse
//! the command with tree-sitter-bash, derive `bash` permission patterns (redirect-inclusive source
//! text), BashArity "always" prefixes, and external directories touched by file-command arguments.
//! Scope: POSIX shells (bash grammar, also used for cmd). PowerShell parsing is a typed gap.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/path.rs
//! |-- tools/src/shell/arity.rs
//! `-- tools/src/shell/select.rs
//! External:
//! `-- tree_sitter::{Node, Parser}
//! Used by:
//! rust/crates/
//! |-- tools/src/shell/legacy.rs
//! `-- tools/src/shell/mod.rs
//! Invariants: every `command` node (including nested substitutions) contributes; cd-like commands
//! never produce bash patterns; dynamic or glob-leading arguments are skipped.
//! Contract tests: tools/tests/shell_scan.rs

use super::arity;
use opencode_platform_api::path;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

const CWD: &[&str] = &[
    "cd",
    "chdir",
    "popd",
    "pushd",
    "push-location",
    "set-location",
];
const FILES: &[&str] = &[
    "rm",
    "cp",
    "mv",
    "mkdir",
    "touch",
    "chmod",
    "chown",
    "cat",
    "get-content",
    "set-content",
    "add-content",
    "copy-item",
    "move-item",
    "remove-item",
    "new-item",
    "rename-item",
];
const CMD_FILES: &[&str] = &[
    "copy", "del", "dir", "erase", "md", "mkdir", "move", "rd", "ren", "rename", "rmdir", "type",
];
const PART_TYPES: &[&str] = &[
    "command_name",
    "command_name_expr",
    "word",
    "string",
    "raw_string",
    "concatenation",
];

/// Insertion-ordered set (JS `Set` iteration order).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ordered(Vec<String>);

impl Ordered {
    pub fn add(&mut self, value: String) {
        if !self.0.contains(&value) {
            self.0.push(value);
        }
    }
    pub fn items(&self) -> &[String] {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub dirs: Ordered,
    pub patterns: Ordered,
    pub always: Ordered,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    #[error("Failed to parse command")]
    Parse,
    #[error(
        "PowerShell command parsing is not ported yet (tree-sitter-powershell grammar missing)"
    )]
    PowerShellNotPorted,
}

/// Instance containment (`containsPath`): directory, or worktree unless it is `/`.
#[derive(Debug, Clone)]
pub struct Instance {
    pub directory: PathBuf,
    pub worktree: PathBuf,
}

impl Instance {
    pub fn contains(&self, target: &Path) -> bool {
        path::contains(&self.directory, target)
            || (self.worktree != Path::new("/") && path::contains(&self.worktree, target))
    }
}

struct Part {
    text: String,
}

fn parts(node: Node, source: &[u8]) -> Vec<Part> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !PART_TYPES.contains(&child.kind()) {
            continue;
        }
        out.push(Part {
            text: child.utf8_text(source).unwrap_or_default().to_string(),
        });
    }
    out
}

fn commands<'a>(root: Node<'a>, out: &mut Vec<Node<'a>>) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "command" {
            out.push(child);
        }
        commands(child, out);
    }
}

fn source_text(node: Node, source: &[u8]) -> String {
    let target = match node.parent() {
        Some(parent) if parent.kind() == "redirected_statement" => parent,
        _ => node,
    };
    target
        .utf8_text(source)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn unquote(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' || first == b'\'') && first == last {
            return &text[1..text.len() - 1];
        }
    }
    text
}

fn home(text: &str, home: &Path) -> String {
    if text == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.join(rest).to_string_lossy().into_owned();
    }
    text.to_string()
}

fn dynamic(text: &str) -> bool {
    text.starts_with('(')
        || text.starts_with("@(")
        || text.contains("$(")
        || text.contains("${")
        || text.contains('`')
        || text.contains('$')
}

/// Path text before the first glob metacharacter; `None` when the glob is leading.
fn prefix(text: &str) -> Option<&str> {
    match text.find(['?', '*', '[']) {
        None => Some(text),
        Some(0) => None,
        Some(index) => Some(&text[..index]),
    }
}

fn path_args(list: &[Part], cmd_shell: bool) -> Vec<String> {
    let first = list.first().map(|part| part.text.as_str());
    list.iter()
        .skip(1)
        .filter(|item| {
            !item.text.starts_with('-')
                && !(cmd_shell && item.text.starts_with('/'))
                && !(first == Some("chmod") && item.text.starts_with('+'))
        })
        .map(|item| item.text.clone())
        .collect()
}

pub fn parse(command: &str) -> Result<tree_sitter::Tree, ScanError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|_| ScanError::Parse)?;
    parser.parse(command, None).ok_or(ScanError::Parse)
}

/// `collect(root, cwd, ps, shell, instance)` for non-PowerShell shells. `shell_name` is
/// `Shell.name(shell)`; `home_dir` is `os.homedir()`.
pub fn collect(
    command: &str,
    cwd: &Path,
    shell_name: &str,
    instance: &Instance,
    home_dir: &Path,
) -> Result<Scan, ScanError> {
    if matches!(shell_name, "pwsh" | "powershell") {
        return Err(ScanError::PowerShellNotPorted);
    }
    let tree = parse(command)?;
    let source = command.as_bytes();
    let cmd_shell = super::select::kind(shell_name) == "cmd";
    let mut scan = Scan::default();
    let mut nodes = Vec::new();
    commands(tree.root_node(), &mut nodes);
    for node in nodes {
        let list = parts(node, source);
        let tokens: Vec<String> = list.iter().map(|part| part.text.clone()).collect();
        let cmd = tokens.first().map(|token| {
            if cmd_shell {
                token.to_lowercase()
            } else {
                token.clone()
            }
        });
        if let Some(cmd) = cmd.as_deref()
            && (FILES.contains(&cmd) || (cmd_shell && CMD_FILES.contains(&cmd)))
        {
            for arg in path_args(&list, cmd_shell) {
                let text = home(unquote(&arg), home_dir);
                let Some(file) = prefix(&text).filter(|file| !file.is_empty()) else {
                    continue;
                };
                if dynamic(file) {
                    continue;
                }
                let resolved = path::resolve(cwd, Path::new(file));
                if instance.contains(&resolved) {
                    continue;
                }
                let dir = if resolved.is_dir() {
                    resolved
                } else {
                    resolved.parent().map(Path::to_path_buf).unwrap_or(resolved)
                };
                scan.dirs.add(dir.to_string_lossy().into_owned());
            }
        }
        if !tokens.is_empty() && !cmd.as_deref().is_some_and(|cmd| CWD.contains(&cmd)) {
            scan.patterns.add(source_text(node, source));
            scan.always
                .add(format!("{} *", arity::prefix(&tokens).join(" ")));
        }
    }
    Ok(scan)
}

/// External-directory approval globs for the scanned directories (`dir/*`).
pub fn globs(scan: &Scan) -> Vec<String> {
    scan.dirs
        .items()
        .iter()
        .map(|dir| Path::new(dir).join("*").to_string_lossy().into_owned())
        .collect()
}

pub fn ordered(values: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    values.into_iter().collect()
}
