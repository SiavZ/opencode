//! Purpose: JavaScript string semantics over UTF-8 storage (UTF-16 indexing, whitespace, quoting).
//! Scope: pure text helpers.
//! Uses:
//! `-- none
//! External:
//! |-- u8::from
//! |-- unicode_normalization::UnicodeNormalization::nfd
//! `-- unicode_normalization::char::is_combining_mark
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/openapi/responses.rs
//! |-- codemode/src/stdlib/array.rs
//! |-- codemode/src/stdlib/json.rs
//! |-- codemode/src/stdlib/regexp.rs
//! |-- codemode/src/stdlib/string.rs
//! |-- codemode/src/tools/discovery.rs
//! |-- codemode/src/tools/render.rs
//! |-- codemode/src/tools/text.rs
//! |-- codemode/src/value/mod.rs
//! `-- codemode/src/value/number.rs
//! Invariants: indexes are UTF-16 code-unit offsets like JS; strings never hold lone surrogates
//! (Rust `String` cannot), so a split surrogate pair renders as U+FFFD.
//! Contract tests: codemode/tests/parity.rs

/// JS `WhiteSpace` and `LineTerminator` (as used by `trim`, `parseInt`, `Number`).
pub(crate) fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

pub(crate) fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

pub(crate) fn from_utf16(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

pub(crate) fn len16(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Clamps a relative JS index (negative counts from the end) into `0..=len`.
pub(crate) fn relative_index(index: f64, len: usize) -> usize {
    let len_f = len as f64;
    let index = if index.is_nan() { 0.0 } else { index.trunc() };
    if index < 0.0 {
        (len_f + index).max(0.0) as usize
    } else {
        index.min(len_f) as usize
    }
}

/// Clamps an absolute JS index (negative -> 0) into `0..=len`.
pub(crate) fn clamp_index(index: f64, len: usize) -> usize {
    let index = if index.is_nan() { 0.0 } else { index.trunc() };
    index.max(0.0).min(len as f64) as usize
}

/// UTF-16 offset to UTF-8 byte offset (clamped).
pub(crate) fn byte_offset(text: &str, unit: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= unit {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

pub(crate) fn slice16(text: &str, start: usize, end: usize) -> String {
    let units = utf16(text);
    let end = end.min(units.len());
    if start >= end {
        return String::new();
    }
    from_utf16(&units[start..end])
}

/// Finds `needle` in `haystack` starting at UTF-16 offset `from`; returns a UTF-16 offset.
pub(crate) fn index_of(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let units = utf16(haystack);
    let needle = utf16(needle);
    if needle.is_empty() {
        return Some(from.min(units.len()));
    }
    if needle.len() > units.len() {
        return None;
    }
    (from..=units.len() - needle.len())
        .find(|start| units[*start..*start + needle.len()] == needle[..])
}

pub(crate) fn last_index_of(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let units = utf16(haystack);
    let needle = utf16(needle);
    if needle.len() > units.len() {
        return None;
    }
    let max_start = (units.len() - needle.len()).min(from);
    (0..=max_start)
        .rev()
        .find(|start| units[*start..*start + needle.len()] == needle[..])
}

/// JSON string quoting exactly like `JSON.stringify(string)`.
pub(crate) fn quote_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Code points of the string as one-character strings (`Array.from(string)`).
pub(crate) fn code_points(text: &str) -> Vec<String> {
    text.chars().map(String::from).collect()
}

/// JS `String.prototype.toLowerCase` / `toUpperCase` (locale-independent).
pub(crate) fn to_lower(text: &str) -> String {
    text.to_lowercase()
}

pub(crate) fn to_upper(text: &str) -> String {
    text.to_uppercase()
}

/// Approximates ICU root-locale collation (`String.prototype.localeCompare`): whitespace and
/// punctuation sort before digits, digits before letters, letters compare case-insensitively
/// first, and lowercase precedes uppercase on a tie.
pub(crate) fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    fn primary(ch: char) -> (u8, String) {
        if ch.is_alphabetic() {
            let base: String = unicode_normalization::UnicodeNormalization::nfd(
                ch.to_lowercase().to_string().as_str(),
            )
            .filter(|item| !unicode_normalization::char::is_combining_mark(*item))
            .collect();
            (3, base)
        } else if ch.is_numeric() {
            (2, ch.to_string())
        } else if ch.is_whitespace() {
            (0, ch.to_string())
        } else {
            (1, ch.to_string())
        }
    }
    let primary_key = |text: &str| text.chars().map(primary).collect::<Vec<_>>();
    primary_key(left).cmp(&primary_key(right)).then_with(|| {
        let tertiary = |text: &str| {
            text.chars()
                .map(|ch| u8::from(ch.is_uppercase()))
                .collect::<Vec<_>>()
        };
        tertiary(left).cmp(&tertiary(right))
    })
}

/// UTF-16 code-unit comparison (the `<` operator on strings).
pub(crate) fn compare16(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}
