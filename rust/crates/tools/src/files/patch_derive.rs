//! Purpose: derive updated file content from parsed update chunks for both generations.
//! Scope: context seeking, four-pass fuzzy line matching, EOF anchoring, replacement splicing,
//! trailing newline and BOM normalization, legacy simplified unified diff.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/common.rs
//! `-- tools/src/files/patch.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- tools/src/files/patch.rs
//! Invariants: seek passes run exact, trimEnd, trim, then Unicode-punctuation normalized, each pass
//! trying the EOF offset before the forward scan. Core pure inserts append at the end, legacy pure
//! inserts land before a remaining trailing blank line.
//! Contract tests: tools/tests/files_patch.rs

use super::super::Generation;
use super::{FileUpdate, PatchError, UpdateFileChunk, js_trim, js_trim_end, split_bom};

type Replacement = (usize, usize, Vec<String>);

pub(super) fn derive(
    generation: Generation,
    path: &str,
    chunks: &[UpdateFileChunk],
    original: &str,
) -> Result<FileUpdate, PatchError> {
    let (bom, text) = split_bom(original);
    let mut lines: Vec<String> = text.split('\n').map(Into::into).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut replacements = compute(generation, &lines, path, chunks)?;
    replacements.sort_by_key(|item| item.0);
    let mut updated = lines;
    for (start, remove, insert) in replacements.into_iter().rev() {
        let start = start.min(updated.len());
        let stop = (start + remove).min(updated.len());
        updated.splice(start..stop, insert);
    }
    if !updated.last().is_some_and(String::is_empty) {
        updated.push(String::new());
    }
    let joined = updated.join("\n");
    let (next_bom, content) = split_bom(&joined);
    Ok(FileUpdate {
        unified_diff: (generation == Generation::Legacy).then(|| unified_diff(text, content)),
        content: content.into(),
        bom: bom || next_bom,
    })
}

fn compute(
    generation: Generation,
    lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<Replacement>, PatchError> {
    let mut replacements = Vec::new();
    let mut line_index = 0;
    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            let Some(found) = seek(lines, std::slice::from_ref(context), line_index, false) else {
                return Err(PatchError::new(format!(
                    "Failed to find context '{context}' in {path}"
                )));
            };
            line_index = found + 1;
        }
        if chunk.old_lines.is_empty() {
            let at = match generation {
                Generation::Legacy if lines.last().is_some_and(String::is_empty) => lines.len() - 1,
                _ => lines.len(),
            };
            replacements.push((at, 0, chunk.new_lines.clone()));
            continue;
        }
        let mut old = chunk.old_lines.as_slice();
        let mut new = chunk.new_lines.as_slice();
        let mut found = seek(lines, old, line_index, chunk.end_of_file);
        if found.is_none() && old.last().is_some_and(String::is_empty) {
            old = &old[..old.len() - 1];
            if new.last().is_some_and(String::is_empty) {
                new = &new[..new.len() - 1];
            }
            found = seek(lines, old, line_index, chunk.end_of_file);
        }
        let Some(found) = found else {
            return Err(PatchError::new(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            )));
        };
        replacements.push((found, old.len(), new.to_vec()));
        line_index = found + old.len();
    }
    Ok(replacements)
}

type Compare = fn(&str, &str) -> bool;

const PASSES: [Compare; 4] = [
    |left, right| left == right,
    |left, right| js_trim_end(left) == js_trim_end(right),
    |left, right| js_trim(left) == js_trim(right),
    |left, right| normalize(js_trim(left)) == normalize(js_trim(right)),
];

fn seek(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() || pattern.len() > lines.len() {
        return None;
    }
    let last = lines.len() - pattern.len();
    let matches = |offset: usize, compare: Compare| {
        pattern
            .iter()
            .zip(&lines[offset..])
            .all(|(want, have)| compare(have, want))
    };
    PASSES.into_iter().find_map(|compare| {
        if eof && last >= start && matches(last, compare) {
            return Some(last);
        }
        (start..=last).find(|offset| matches(*offset, compare))
    })
}

fn normalize(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => output.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => output.push('"'),
            '\u{2010}'..='\u{2015}' => output.push('-'),
            '\u{2026}' => output.push_str("..."),
            '\u{00A0}' => output.push(' '),
            other => output.push(other),
        }
    }
    output
}

/// Legacy `generateUnifiedDiff`: positional line comparison under a fixed `@@ -1 +1 @@` header,
/// empty lines are dropped from `-`/`+`/context output, empty string when nothing differs.
pub fn unified_diff(old: &str, new: &str) -> String {
    let old: Vec<&str> = old.split('\n').collect();
    let new: Vec<&str> = new.split('\n').collect();
    let mut diff = String::from("@@ -1 +1 @@\n");
    let mut changed = false;
    for index in 0..old.len().max(new.len()) {
        let before = old.get(index).copied().unwrap_or("");
        let after = new.get(index).copied().unwrap_or("");
        if before != after {
            if !before.is_empty() {
                diff.push_str(&format!("-{before}\n"));
            }
            if !after.is_empty() {
                diff.push_str(&format!("+{after}\n"));
            }
            changed = true;
            continue;
        }
        if !before.is_empty() {
            diff.push_str(&format!(" {before}\n"));
        }
    }
    if changed { diff } else { String::new() }
}
