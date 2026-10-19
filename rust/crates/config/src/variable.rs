//! Purpose: `{env:VAR}` and `{file:path}` substitution for config text
//! (`packages/opencode/src/config/variable.ts:25-91`), including `~/` expansion, relative
//! resolution against the config directory, and skipping tokens on `//` comment lines.
//! Scope: pure text transform plus file reads.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! `-- platform-api/src/path.rs
//! External:
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/steps.rs
//! |-- config/src/lib.rs
//! |-- config/src/resources.rs
//! `-- config/tests/markdown.rs
//! Invariants: env substitution runs first over the whole text; included file content is trimmed
//! and JSON-string escaped; substituted values never flow back into written files (callers write
//! the original text only).
//! Contract tests: config/tests/markdown.rs

use crate::env::Environment;
use crate::error::ConfigError;
use opencode_platform_api::path::resolve;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the text came from: a file path, or a virtual source with a base directory.
#[derive(Debug, Clone)]
pub enum Source {
    Path(PathBuf),
    Virtual { source: String, dir: PathBuf },
}

impl Source {
    pub fn name(&self) -> String {
        match self {
            Self::Path(path) => path.display().to_string(),
            Self::Virtual { source, .. } => source.clone(),
        }
    }

    fn dir(&self) -> PathBuf {
        match self {
            Self::Path(path) => path.parent().map(Path::to_path_buf).unwrap_or_default(),
            Self::Virtual { dir, .. } => dir.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    Error,
    Empty,
}

/// Apply `{env:VAR}` then `{file:path}` substitutions. `overlay` wins over process env
/// (`input.env?.[name] ?? process.env[name]`), and empty values become `""`.
pub fn substitute(
    text: &str,
    source: &Source,
    env: &Environment,
    overlay: Option<&BTreeMap<String, String>>,
    missing: Missing,
) -> Result<String, ConfigError> {
    let text = substitute_env(text, env, overlay);
    if !text.contains("{file:") {
        return Ok(text);
    }
    let dir = source.dir();
    let mut out = String::new();
    let mut cursor = 0;
    for (index, token) in file_tokens(&text) {
        out.push_str(&text[cursor..index]);
        cursor = index + token.len();
        let line_start = text[..index].rfind('\n').map(|at| at + 1).unwrap_or(0);
        if text[line_start..index].trim_start().starts_with("//") {
            out.push_str(token);
            continue;
        }
        let raw = &token["{file:".len()..token.len() - 1];
        let expanded = match raw.strip_prefix("~/") {
            Some(rest) => env.home.join(rest),
            None => PathBuf::from(raw),
        };
        let resolved = resolve(&dir, &expanded);
        let content = match std::fs::read(&resolved) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) if missing == Missing::Empty => String::new(),
            Err(error) => {
                let message = format!("bad file reference: \"{token}\"");
                let message = if error.kind() == std::io::ErrorKind::NotFound {
                    format!("{message} {} does not exist", resolved.display())
                } else {
                    message
                };
                return Err(ConfigError::invalid_message(source.name(), message));
            }
        };
        let escaped = serde_json::to_string(js_trim(&content)).unwrap_or_default();
        out.push_str(&escaped[1..escaped.len() - 1]);
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

fn substitute_env(
    text: &str,
    env: &Environment,
    overlay: Option<&BTreeMap<String, String>>,
) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{env:") {
        let after = &rest[start + 5..];
        match after.find('}') {
            Some(end) if end > 0 => {
                out.push_str(&rest[..start]);
                let name = &after[..end];
                let value = overlay
                    .and_then(|overlay| overlay.get(name))
                    .map(String::as_str)
                    .or_else(|| env.var(name));
                out.push_str(value.unwrap_or(""));
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 5]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Non-overlapping `/\{file:[^}]+\}/g` matches as (byte index, token).
fn file_tokens(text: &str) -> Vec<(usize, &str)> {
    let mut tokens = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("{file:") {
        let start = from + found;
        let body = &text[start + 6..];
        match body.find('}') {
            Some(end) if end > 0 => {
                let token = &text[start..start + 6 + end + 1];
                tokens.push((start, token));
                from = start + token.len();
            }
            _ => from = start + 1,
        }
    }
    tokens
}

/// `String.prototype.trim` (JS whitespace + line terminators, including U+FEFF).
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
}
