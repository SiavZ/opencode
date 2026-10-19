//! Purpose: ordering that matches JavaScript `String.prototype.localeCompare` with no locale argument
//! (ICU root collation, non-ignorable punctuation), as used by the legacy JSON store list sort and
//! `Workspace.list`.
//! Scope: pure comparison.
//! Uses:
//! `-- none
//! External:
//! `-- u8::from
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! |-- storage-api/src/records/mod.rs
//! |-- storage-sqlite/src/legacy_files/store.rs
//! `-- storage-sqlite/src/repositories/domains.rs
//! Invariants: printable ASCII follows the primary order captured from bun 1.3 (`ASCII_ORDER`), letters
//! compare case-insensitively at the primary level and lowercase sorts first at the tertiary level.
//! Control characters are ignorable. Non-ASCII code points sort after all ASCII by code point, a
//! documented approximation (keys and ids are ASCII in practice).
//! Contract tests: storage-api/src/records/collation.rs

use std::cmp::Ordering;

/// Printable ASCII sorted with `localeCompare` under bun 1.3 (ICU root). Letters appear as `aA`.
pub const ASCII_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";

fn weights(c: char) -> (u32, u8) {
    if c.is_ascii_alphabetic() {
        let index = ASCII_ORDER.find(c.to_ascii_lowercase()).unwrap_or(0) as u32;
        return (index, u8::from(c.is_ascii_uppercase()));
    }
    if let Some(index) = ASCII_ORDER.find(c) {
        return (index as u32, 0);
    }
    (1000 + c as u32, 0)
}

fn key(text: &str) -> Vec<(u32, u8)> {
    text.chars()
        .filter(|c| (*c as u32) >= 0x20 && *c != '\u{7f}')
        .map(weights)
        .collect()
}

/// Compare like `a.localeCompare(b)`.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let (wa, wb) = (key(a), key(b));
    wa.iter()
        .map(|w| w.0)
        .cmp(wb.iter().map(|w| w.0))
        .then_with(|| wa.iter().map(|w| w.1).cmp(wb.iter().map(|w| w.1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected orders captured with bun 1.3: `[...].sort((a, b) => a.localeCompare(b))`.
    #[test]
    fn matches_bun_samples() {
        let mut words = vec![
            "B", "a10", "Ab", "a_b", "b", "a", "aB", "a2", "ab", "a-b", "A", "a1",
        ];
        words.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(
            words,
            [
                "a", "A", "a_b", "a-b", "a1", "a10", "a2", "ab", "aB", "Ab", "b", "B"
            ]
        );
        let mut keys = vec!["ses/a", "ses.a", "ses-a", "ses_aa", "ses_a/x", "ses_a"];
        keys.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(
            keys,
            ["ses_a", "ses_a/x", "ses_aa", "ses-a", "ses.a", "ses/a"]
        );
    }
}
