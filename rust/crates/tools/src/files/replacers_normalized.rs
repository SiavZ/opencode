//! Purpose: normalized whitespace, indentation, escapes and boundary candidate strategies.
//! Scope: pure text matching.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/replacers.rs
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! `-- tools/src/files/replacers.rs
//! Invariants: normalization yields original spans, not normalized replacement text.
//! Contract tests: tools/tests/files_replacers.rs

use super::{drop_utf16, indent_len, js_whitespace, trim};
use regex::Regex;

const SPACE: &str = r"[\t\n\x0B\x0C\r \u{00A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";

fn normalize(value: &str) -> String {
    value
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn whitespace(content: &str, find: &str) -> Vec<String> {
    let desired = normalize(find);
    let lines: Vec<_> = content.split('\n').collect();
    let words: Vec<_> = trim(find)
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect();
    let pattern = words
        .iter()
        .map(|word| regex::escape(word))
        .collect::<Vec<_>>()
        .join(&format!("{SPACE}+"));
    let regex = Regex::new(&pattern).ok();
    let mut result = Vec::new();
    for line in &lines {
        let actual = normalize(line);
        if actual == desired {
            result.push((*line).into());
            continue;
        }
        if actual.contains(&desired)
            && let Some(found) = regex.as_ref().and_then(|regex| regex.find(line))
        {
            result.push(found.as_str().into());
        }
    }
    let count = find.split('\n').count();
    if count > 1 {
        result.extend(
            lines
                .windows(count)
                .map(|block| block.join("\n"))
                .filter(|block| normalize(block) == desired),
        );
    }
    result
}

fn remove_indentation(value: &str) -> String {
    let lines: Vec<_> = value.split('\n').collect();
    let Some(minimum) = lines
        .iter()
        .filter(|line| !trim(line).is_empty())
        .map(|line| indent_len(line))
        .min()
    else {
        return value.into();
    };
    lines
        .iter()
        .map(|line| {
            if trim(line).is_empty() {
                *line
            } else {
                drop_utf16(line, minimum)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn indentation(content: &str, find: &str) -> Vec<String> {
    let desired = remove_indentation(find);
    content
        .split('\n')
        .collect::<Vec<_>>()
        .windows(find.split('\n').count())
        .map(|block| block.join("\n"))
        .filter(|block| remove_indentation(block) == desired)
        .collect()
}

fn unescape(value: &str) -> String {
    let mut chars = value.chars().peekable();
    let mut result = String::new();
    while let Some(character) = chars.next() {
        if character == '\\'
            && let Some(next) = chars.peek()
        {
            let decoded = match next {
                'n' => Some('\n'),
                't' => Some('\t'),
                'r' => Some('\r'),
                '\'' | '"' | '`' | '\\' | '\n' | '$' => Some(*next),
                _ => None,
            };
            if let Some(decoded) = decoded {
                result.push(decoded);
                chars.next();
                continue;
            }
        }
        result.push(character);
    }
    result
}

pub(super) fn escape(content: &str, find: &str) -> Vec<String> {
    let desired = unescape(find);
    let mut result = Vec::new();
    if content.contains(&desired) {
        result.push(desired.clone());
    }
    result.extend(
        content
            .split('\n')
            .collect::<Vec<_>>()
            .windows(desired.split('\n').count())
            .map(|block| block.join("\n"))
            .filter(|block| unescape(block) == desired),
    );
    result
}

pub(super) fn boundary(content: &str, find: &str) -> Vec<String> {
    let desired = trim(find);
    if desired == find {
        return vec![];
    }
    let mut result = Vec::new();
    if content.contains(desired) {
        result.push(desired.into());
    }
    result.extend(
        content
            .split('\n')
            .collect::<Vec<_>>()
            .windows(find.split('\n').count())
            .map(|block| block.join("\n"))
            .filter(|block| trim(block) == desired),
    );
    result
}
