//! Purpose: ordered legacy fuzzy edit matching and disproportionate-span safety checks.
//! Scope: pure text transformations, no filesystem or permission effects.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/definition.rs
//! |-- tools/src/files/replacers_anchors.rs
//! `-- tools/src/files/replacers_normalized.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_replacers.rs
//! Invariants: strategy order, ambiguity errors and 0.65 anchor thresholds match legacy edit.ts.
//! Contract tests: tools/tests/files_replacers.rs

#[path = "replacers_anchors.rs"]
mod anchors;
#[path = "replacers_normalized.rs"]
mod normalized;

use opencode_tool_api::ToolFailure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Simple,
    LineTrimmed,
    BlockAnchor,
    WhitespaceNormalized,
    IndentationFlexible,
    EscapeNormalized,
    TrimmedBoundary,
    ContextAware,
    MultiOccurrence,
}

pub const STRATEGIES: [Strategy; 9] = [
    Strategy::Simple,
    Strategy::LineTrimmed,
    Strategy::BlockAnchor,
    Strategy::WhitespaceNormalized,
    Strategy::IndentationFlexible,
    Strategy::EscapeNormalized,
    Strategy::TrimmedBoundary,
    Strategy::ContextAware,
    Strategy::MultiOccurrence,
];

pub fn candidates(strategy: Strategy, content: &str, find: &str) -> Vec<String> {
    match strategy {
        Strategy::Simple => vec![find.into()],
        Strategy::LineTrimmed => line_trimmed(content, find),
        Strategy::BlockAnchor => anchors::block_anchor(content, find),
        Strategy::WhitespaceNormalized => normalized::whitespace(content, find),
        Strategy::IndentationFlexible => normalized::indentation(content, find),
        Strategy::EscapeNormalized => normalized::escape(content, find),
        Strategy::TrimmedBoundary => normalized::boundary(content, find),
        Strategy::ContextAware => anchors::context_aware(content, find),
        Strategy::MultiOccurrence => {
            // Empty searches are rejected by replace before strategies run. Avoid the baseline's
            // non-terminating generator if a direct strategy caller supplies an empty search.
            if find.is_empty() {
                return vec![];
            }
            content.match_indices(find).map(|_| find.into()).collect()
        }
    }
}

pub fn replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, ToolFailure> {
    if old == new {
        return Err(ToolFailure::new(
            "No changes to apply: oldString and newString are identical.",
        ));
    }
    if old.is_empty() {
        return Err(ToolFailure::new(
            "oldString cannot be empty when editing an existing file. Provide the exact text to replace, or use write for an intentional full-file replacement.",
        ));
    }
    let mut found = false;
    for strategy in STRATEGIES {
        for search in candidates(strategy, content, old) {
            let Some(index) = content.find(&search) else {
                continue;
            };
            found = true;
            if disproportionate(&search, old) {
                return Err(ToolFailure::new(
                    "Refusing replacement because the matched span is much larger than oldString. Re-read the file and provide the full exact oldString for the intended replacement.",
                ));
            }
            if replace_all {
                return Ok(replace_all_text(content, &search, new));
            }
            if Some(index) != content.rfind(&search) {
                continue;
            }
            return Ok(format!(
                "{}{}{}",
                &content[..index],
                new,
                &content[index + search.len()..]
            ));
        }
    }
    Err(ToolFailure::new(if found {
        "Found multiple matches for oldString. Provide more surrounding context to make the match unique."
    } else {
        "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings."
    }))
}

fn replace_all_text(content: &str, search: &str, replacement: &str) -> String {
    js_string_replace(content, search, replacement, true)
}

/// JS `String.prototype.replace`/`replaceAll` with a string pattern and string replacement,
/// including `$$`, `$&`, `` $` `` and `$'` substitution (no capture groups exist for strings).
pub(crate) fn js_string_replace(
    content: &str,
    search: &str,
    replacement: &str,
    all: bool,
) -> String {
    let mut result = String::new();
    let mut offset = 0;
    let matches: Vec<_> = if all {
        content.match_indices(search).collect()
    } else {
        content.match_indices(search).take(1).collect()
    };
    for (index, matched) in matches {
        result.push_str(&content[offset..index]);
        let mut chars = replacement.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '$' {
                match chars.peek() {
                    Some('$') => {
                        result.push('$');
                        chars.next();
                        continue;
                    }
                    Some('&') => {
                        result.push_str(matched);
                        chars.next();
                        continue;
                    }
                    Some('`') => {
                        result.push_str(&content[..index]);
                        chars.next();
                        continue;
                    }
                    Some('\'') => {
                        result.push_str(&content[index + matched.len()..]);
                        chars.next();
                        continue;
                    }
                    _ => {}
                }
            }
            result.push(character);
        }
        offset = index + matched.len();
    }
    result.push_str(&content[offset..]);
    result
}

fn disproportionate(search: &str, old: &str) -> bool {
    let old_lines = old.split('\n').count();
    let search_lines = search.split('\n').count();
    if search_lines >= (old_lines + 3).max(old_lines * 2) {
        return true;
    }
    if old_lines == 1 {
        return false;
    }
    utf16_len(trim(search)) > (utf16_len(trim(old)) + 500).max(utf16_len(trim(old)) * 4)
}

fn line_trimmed(content: &str, find: &str) -> Vec<String> {
    let lines: Vec<_> = content.split('\n').collect();
    let mut search: Vec<_> = find.split('\n').collect();
    if search.last() == Some(&"") {
        search.pop();
    }
    if search.is_empty() {
        return (0..=lines.len()).map(|_| String::new()).collect();
    }
    lines
        .windows(search.len())
        .filter(|block| block.iter().zip(&search).all(|(a, b)| trim(a) == trim(b)))
        .map(|block| block.join("\n"))
        .collect()
}

pub fn trim_diff(diff: &str) -> String {
    let lines: Vec<_> = diff.split('\n').collect();
    let is_content = |line: &&str| {
        matches!(line.chars().next(), Some('+' | '-' | ' '))
            && !line.starts_with("---")
            && !line.starts_with("+++")
    };
    let minimum = lines
        .iter()
        .filter(|line| is_content(line))
        .map(|line| &line[1..])
        .filter(|line| !trim(line).is_empty())
        .map(indent_len)
        .min()
        .unwrap_or(0);
    if minimum == 0 {
        return diff.into();
    }
    lines
        .iter()
        .map(|line| {
            if !is_content(line) {
                return (*line).into();
            }
            format!("{}{}", &line[..1], drop_utf16(&line[1..], minimum))
        })
        .collect::<Vec<String>>()
        .join("\n")
}

pub(super) fn trim(value: &str) -> &str {
    value.trim_matches(js_whitespace)
}

pub(super) fn js_whitespace(value: char) -> bool {
    matches!(
        value,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

pub(super) fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub(super) fn indent_len(value: &str) -> usize {
    value
        .chars()
        .take_while(|c| js_whitespace(*c))
        .map(char::len_utf16)
        .sum()
}

pub(super) fn drop_utf16(value: &str, count: usize) -> &str {
    let mut units = 0;
    for (index, character) in value.char_indices() {
        if units >= count {
            return &value[index..];
        }
        units += character.len_utf16();
    }
    ""
}
