//! Purpose: legacy block-anchor and context-aware candidate matching.
//! Scope: pure line matching and UTF-16 Levenshtein scoring.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/replacers.rs
//! External:
//! `-- usize::from
//! Used by:
//! rust/crates/
//! `-- tools/src/files/replacers.rs
//! Invariants: first closing anchor wins, candidate size delta is max(1,floor(lines*0.25)).
//! Contract tests: tools/tests/files_replacers.rs

use super::{trim, utf16_len};

pub(super) fn block_anchor(content: &str, find: &str) -> Vec<String> {
    let lines: Vec<_> = content.split('\n').collect();
    let mut search: Vec<_> = find.split('\n').collect();
    if search.len() < 3 {
        return vec![];
    }
    if search.last() == Some(&"") {
        search.pop();
    }
    let maximum_delta = (search.len() / 4).max(1);
    let mut positions = Vec::new();
    for start in 0..lines.len() {
        if trim(lines[start]) != trim(search[0]) {
            continue;
        }
        for (end, line) in lines.iter().enumerate().skip(start + 2) {
            if trim(line) != trim(search[search.len() - 1]) {
                continue;
            }
            if (end - start + 1).abs_diff(search.len()) <= maximum_delta {
                positions.push((start, end));
            }
            break;
        }
    }
    let single = positions.len() == 1;
    let mut best = None;
    let mut maximum = -1.0;
    for (start, end) in positions {
        let count = (search.len() - 2).min(end - start - 1);
        let mut similarity = if count == 0 { 1.0 } else { 0.0 };
        for middle in 1..=count {
            let original = trim(lines[start + middle]);
            let desired = trim(search[middle]);
            let length = utf16_len(original).max(utf16_len(desired));
            if length == 0 {
                continue;
            }
            let score = 1.0 - levenshtein(original, desired) as f64 / length as f64;
            similarity += if single { score / count as f64 } else { score };
            if single && similarity >= 0.65 {
                break;
            }
        }
        if !single && count > 0 {
            similarity /= count as f64;
        }
        if similarity > maximum {
            maximum = similarity;
            best = Some((start, end));
        }
    }
    if maximum < 0.65 {
        return vec![];
    }
    best.map(|(start, end)| vec![lines[start..=end].join("\n")])
        .unwrap_or_default()
}

pub(super) fn context_aware(content: &str, find: &str) -> Vec<String> {
    let lines: Vec<_> = content.split('\n').collect();
    let mut search: Vec<_> = find.split('\n').collect();
    if search.len() < 3 {
        return vec![];
    }
    if search.last() == Some(&"") {
        search.pop();
    }
    let mut result = Vec::new();
    for start in 0..lines.len() {
        if trim(lines[start]) != trim(search[0]) {
            continue;
        }
        for (end, line) in lines.iter().enumerate().skip(start + 2) {
            if trim(line) != trim(search[search.len() - 1]) {
                continue;
            }
            if end - start + 1 == search.len() {
                let mut matching = 0;
                let mut nonempty = 0;
                for middle in 1..search.len() - 1 {
                    let original = trim(lines[start + middle]);
                    let desired = trim(search[middle]);
                    if !original.is_empty() || !desired.is_empty() {
                        nonempty += 1;
                        if original == desired {
                            matching += 1;
                        }
                    }
                }
                if nonempty == 0 || matching as f64 / nonempty as f64 >= 0.5 {
                    result.push(lines[start..=end].join("\n"));
                }
            }
            break;
        }
    }
    result
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<_> = a.encode_utf16().collect();
    let b: Vec<_> = b.encode_utf16().collect();
    let mut previous: Vec<_> = (0..=b.len()).collect();
    for (i, left) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, right) in b.iter().enumerate() {
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + usize::from(left != right));
        }
        previous = current;
    }
    previous[b.len()]
}
