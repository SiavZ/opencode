//! Purpose: pure apply_patch engines for both tool generations: parse patch text into hunks and
//! derive updated file content from update chunks. Does not touch the filesystem.
//! Scope: public patch API, shared types, BOM and heredoc helpers, legacy shell argv detection.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/patch_derive.rs
//! |-- tools/src/files/patch_parse.rs
//! `-- tools/src/files/replacers.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_patch.rs
//! Invariants: Core mirrors packages/core/src/patch.ts (strict syntax errors), Legacy mirrors
//! packages/opencode/src/patch/index.ts (lenient skipping, end-of-file marker never recorded, pure
//! inserts land before a trailing blank line, simplified unified diff). JS `trim` whitespace.
//! Contract tests: tools/tests/files_patch.rs

#[path = "patch_derive.rs"]
mod derive;
#[path = "patch_parse.rs"]
mod parse;

use super::Generation;
use super::replacers::js_whitespace;

pub use derive::unified_diff;

/// One file operation. Core `movePath`/legacy `move_path` share `move_path`; legacy keeps an empty
/// `*** Move to:` value as `Some("")` while Core rejects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hunk {
    Add {
        path: String,
        contents: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    pub fn path(&self) -> &str {
        match self {
            Hunk::Add { path, .. } | Hunk::Delete { path } | Hunk::Update { path, .. } => path,
        }
    }
}

/// `@@` chunk. `end_of_file` is TS `endOfFile`/`is_end_of_file` (`true | undefined`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateFileChunk {
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub change_context: Option<String>,
    pub end_of_file: bool,
}

/// Derived update. `content` never carries a BOM; `bom` records whether the original or the
/// result had one. `unified_diff` is only produced by the legacy engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUpdate {
    pub content: String,
    pub bom: bool,
    pub unified_diff: Option<String>,
}

/// Engine failure. `message` is the TS `Error.message`; tools that interpolate the error with
/// `String(error)` or `${error}` must use [`PatchError::js_string`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PatchError {
    pub message: String,
}

impl PatchError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// JavaScript `String(new Error(message))`.
    pub fn js_string(&self) -> String {
        format!("Error: {}", self.message)
    }
}

/// `Patch.parse` (Core) or `Patch.parsePatch(...).hunks` (Legacy).
pub fn parse(generation: Generation, patch_text: &str) -> Result<Vec<Hunk>, PatchError> {
    let cleaned = strip_heredoc(js_trim(patch_text));
    let lines: Vec<&str> = cleaned.split('\n').collect();
    let begin = lines.iter().position(|l| js_trim(l) == "*** Begin Patch");
    let end = lines.iter().position(|l| js_trim(l) == "*** End Patch");
    let (Some(begin), Some(end)) = (begin, end) else {
        return Err(PatchError::new(
            "Invalid patch format: missing Begin/End markers",
        ));
    };
    if begin >= end {
        return Err(PatchError::new(
            "Invalid patch format: missing Begin/End markers",
        ));
    }
    match generation {
        Generation::Core => parse::core(&lines, begin, end),
        Generation::Legacy => Ok(parse::legacy(&lines, begin, end)),
    }
}

/// `Patch.derive` (Core) or `Patch.deriveNewContentsFromChunks` (Legacy).
pub fn derive(
    generation: Generation,
    path: &str,
    chunks: &[UpdateFileChunk],
    original: &str,
) -> Result<FileUpdate, PatchError> {
    derive::derive(generation, path, chunks, original)
}

/// `{ bom, text }` split on a leading U+FEFF.
pub fn split_bom(text: &str) -> (bool, &str) {
    match text.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

/// `Patch.joinBom` / legacy `Bom.join`: strip one leading BOM, then prepend one when requested.
pub fn join_bom(text: &str, bom: bool) -> String {
    let stripped = split_bom(text).1;
    if bom {
        format!("\u{FEFF}{stripped}")
    } else {
        stripped.to_string()
    }
}

/// Legacy `MaybeApplyPatch` result for shell-style invocations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaybeApplyPatch {
    Body { patch: String, hunks: Vec<Hunk> },
    PatchParseError(PatchError),
    NotApplyPatch,
}

/// Legacy `maybeParseApplyPatch`: `apply_patch <patch>`, `applypatch <patch>` or
/// `bash -lc 'apply_patch <<"EOF" ...'` (quoted delimiter required, unanchored match).
pub fn maybe_parse_apply_patch(argv: &[&str]) -> MaybeApplyPatch {
    let body = match argv {
        [command, patch] if *command == "apply_patch" || *command == "applypatch" => Some(*patch),
        ["bash", "-lc", script] => bash_heredoc(script),
        _ => None,
    };
    let Some(patch) = body else {
        return MaybeApplyPatch::NotApplyPatch;
    };
    match parse(Generation::Legacy, patch) {
        Ok(hunks) => MaybeApplyPatch::Body {
            patch: patch.to_string(),
            hunks,
        },
        Err(error) => MaybeApplyPatch::PatchParseError(error),
    }
}

pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(js_whitespace)
}

pub(crate) fn js_trim_end(value: &str) -> &str {
    value.trim_end_matches(js_whitespace)
}

/// `/^(?:cat\s+)?<<['"]?(\w+)['"]?\s*\n([\s\S]*?)\n\1\s*$/` with JS backtracking semantics.
fn strip_heredoc(input: &str) -> &str {
    let rest = input
        .strip_prefix("cat")
        .filter(|rest| rest.starts_with(js_whitespace))
        .map(|rest| rest.trim_start_matches(js_whitespace))
        .unwrap_or(input);
    let Some(rest) = rest.strip_prefix("<<") else {
        return input;
    };
    let rest = rest.strip_prefix(['\'', '"']).unwrap_or(rest);
    let Some((tag, rest)) = word(rest) else {
        return input;
    };
    let rest = rest.strip_prefix(['\'', '"']).unwrap_or(rest);
    heredoc_body(rest, tag, true).unwrap_or(input)
}

/// `/apply_patch\s*<<['"](\w+)['"]\s*\n([\s\S]*?)\n\1/` leftmost match, capture 2.
fn bash_heredoc(script: &str) -> Option<&str> {
    script.match_indices("apply_patch").find_map(|(index, _)| {
        let rest = script[index + "apply_patch".len()..].trim_start_matches(js_whitespace);
        let rest = rest.strip_prefix("<<")?.strip_prefix(['\'', '"'])?;
        let (tag, rest) = word(rest)?;
        heredoc_body(rest.strip_prefix(['\'', '"'])?, tag, false)
    })
}

/// ASCII `\w+`, greedy. Shorter backtracking never helps because the next token is not `\w`.
fn word(input: &str) -> Option<(&str, &str)> {
    let length = input
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    (length > 0).then(|| input.split_at(length))
}

/// Matches `\s*\n([\s\S]*?)\n<tag>` (plus `\s*$` when `anchored`) at the start of `rest`.
/// The greedy `\s*` backtracks over each newline in the leading whitespace, last first.
fn heredoc_body<'a>(rest: &'a str, tag: &str, anchored: bool) -> Option<&'a str> {
    let run = rest.len() - rest.trim_start_matches(js_whitespace).len();
    let closing = format!("\n{tag}");
    rest[..run]
        .match_indices('\n')
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|start| {
            let body = &rest[start..];
            body.match_indices(closing.as_str())
                .map(|(end, _)| end)
                .find(|end| !anchored || body[end + closing.len()..].chars().all(js_whitespace))
                .map(|end| &body[..end])
        })
}
