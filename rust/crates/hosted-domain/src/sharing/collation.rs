//! Purpose: ordering that matches JavaScript `String.prototype.localeCompare` (ICU root collation,
//! non-ignorable punctuation) for the ASCII share keys sorted by enterprise `Share.merge`.
//! Scope: pure comparison, no allocation beyond weight vectors.
//! Uses:
//! `-- none
//! External:
//! `-- u8::from
//! Used by:
//! rust/crates/
//! |-- hosted-domain/src/sharing/mod.rs
//! |-- hosted-domain/src/sharing/share.rs
//! `-- hosted-domain/tests/sharing_domain.rs
//! Invariants: for printable ASCII the primary order is the ICU root order captured from bun 1.3
//! (`ASCII_ORDER`), letters compare case-insensitively at the primary level and lowercase sorts
//! before uppercase at the tertiary level. Non-ASCII code points sort after all ASCII by code point
//! (documented approximation, share keys are ASCII identifiers in practice).
//! Contract tests: hosted-domain/tests/sharing_domain.rs

use std::cmp::Ordering;

/// Printable ASCII sorted with `localeCompare` under bun 1.3.14 (ICU root). Letters appear as
/// `aA`, both share one primary weight.
pub const ASCII_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";

fn weights(c: char) -> (u32, u8) {
    if c.is_ascii_alphabetic() {
        let lower = c.to_ascii_lowercase();
        let index = ASCII_ORDER.find(lower).unwrap_or(0) as u32;
        return (index, u8::from(c.is_ascii_uppercase()));
    }
    if let Some(index) = ASCII_ORDER.find(c) {
        return (index as u32, 0);
    }
    if (c as u32) < 0x20 {
        // Control characters are completely ignorable in ICU root collation.
        return (0, 0);
    }
    (1000 + c as u32, 0)
}

/// Compare like `a.localeCompare(b)` in the baseline runtime.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let wa: Vec<(u32, u8)> = a
        .chars()
        .filter(|c| (*c as u32) >= 0x20)
        .map(weights)
        .collect();
    let wb: Vec<(u32, u8)> = b
        .chars()
        .filter(|c| (*c as u32) >= 0x20)
        .map(weights)
        .collect();
    let primary = wa.iter().map(|w| w.0).cmp(wb.iter().map(|w| w.0));
    if primary != Ordering::Equal {
        return primary;
    }
    wa.iter().map(|w| w.1).cmp(wb.iter().map(|w| w.1))
}
