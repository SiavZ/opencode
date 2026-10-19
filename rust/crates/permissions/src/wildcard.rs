//! Purpose: wildcard matching identical to packages/core/src/util/wildcard.ts and the legacy
//! packages/opencode/src/util/wildcard.ts (`match`, `all`, `allStructured`).
//! Scope: pure string matching.
//! Uses:
//! `-- none
//! External:
//! |-- regex::Regex
//! `-- regex::escape
//! Used by:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! |-- permissions/src/rule.rs
//! |-- permissions/src/service.rs
//! `-- session-runtime/src/legacy/parts.rs
//! Invariants: `*` matches any run including `/` and newlines, `?` one char, backslashes are
//! normalized to `/` on both sides, a trailing ` *` also matches the bare prefix. Case-insensitive
//! only on Windows.
//! Contract tests: permissions/src/wildcard.rs

use regex::Regex;

/// Compile a baseline wildcard pattern to an anchored regex.
pub fn compile(pattern: &str) -> Regex {
    let pattern = pattern.replace('\\', "/");
    let mut escaped = String::with_capacity(pattern.len() * 2);
    for ch in pattern.chars() {
        match ch {
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            '*' => escaped.push_str(".*"),
            '?' => escaped.push('.'),
            other => escaped.push_str(&regex::escape(&other.to_string())),
        }
    }
    if let Some(prefix) = escaped.strip_suffix(" .*") {
        escaped = format!("{prefix}( .*)?");
    }
    let flags = if cfg!(windows) { "(?si)" } else { "(?s)" };
    // Every metacharacter is escaped above, so the pattern always compiles.
    Regex::new(&format!("{flags}^{escaped}$")).expect("escaped wildcard compiles")
}

/// `Wildcard.match(input, pattern)`.
pub fn matches(input: &str, pattern: &str) -> bool {
    compile(pattern).is_match(&input.replace('\\', "/"))
}

/// Legacy `Wildcard.all`: patterns sorted by length then lexically, the last match wins.
pub fn all<'a, V>(input: &str, patterns: &'a [(String, V)]) -> Option<&'a V> {
    let mut sorted: Vec<_> = patterns.iter().collect();
    sorted.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
    sorted
        .into_iter()
        .filter(|(pattern, _)| matches(input, pattern))
        .map(|(_, value)| value)
        .last()
}

/// Legacy `Wildcard.allStructured` over a command head and argument tail.
pub fn all_structured<'a, V>(
    head: &str,
    tail: &[String],
    patterns: &'a [(String, V)],
) -> Option<&'a V> {
    let mut sorted: Vec<_> = patterns.iter().collect();
    sorted.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
    sorted
        .into_iter()
        .filter(|(pattern, _)| {
            let parts: Vec<&str> = pattern.split_whitespace().collect();
            let first = parts.first().copied().unwrap_or("");
            matches(head, first) && (parts.len() <= 1 || match_sequence(tail, &parts[1..]))
        })
        .map(|(_, value)| value)
        .last()
}

fn match_sequence(items: &[String], patterns: &[&str]) -> bool {
    let Some((pattern, rest)) = patterns.split_first() else {
        return true;
    };
    if *pattern == "*" {
        return match_sequence(items, rest);
    }
    (0..items.len()).any(|i| matches(&items[i], pattern) && match_sequence(&items[i + 1..], rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_and_question() {
        assert!(matches("src/a/b.ts", "src/*"));
        assert!(matches("abc", "a?c"));
        assert!(!matches("abbc", "a?c"));
        assert!(matches("line1\nline2", "line1*"));
        assert!(matches("a.b", "a.b"));
        assert!(!matches("axb", "a.b"));
        assert!(matches("(x)[y]{z}|$^+", "(x)[y]{z}|$^+"));
    }

    #[test]
    fn trailing_space_star_is_optional() {
        assert!(matches("ls", "ls *"));
        assert!(matches("ls -la", "ls *"));
        assert!(!matches("lsof", "ls *"));
    }

    #[test]
    fn backslashes_normalize() {
        assert!(matches("C:\\Users\\x", "C:/Users/*"));
        assert!(matches("C:/Users/x", "C:\\Users\\*"));
    }

    #[test]
    fn all_prefers_longest_then_lexical_last() {
        let patterns = vec![
            ("*".to_string(), "ask"),
            ("git *".to_string(), "allow"),
            ("git push *".to_string(), "deny"),
        ];
        assert_eq!(all("git push origin", &patterns), Some(&"deny"));
        assert_eq!(all("git status", &patterns), Some(&"allow"));
        assert_eq!(all("rm -rf", &patterns), Some(&"ask"));
    }

    #[test]
    fn structured_sequence() {
        let patterns = vec![("git * --force".to_string(), 1), ("git".to_string(), 0)];
        let tail = vec!["push".to_string(), "--force".to_string()];
        assert_eq!(all_structured("git", &tail, &patterns), Some(&1));
        assert_eq!(
            all_structured("git", &["status".to_string()], &patterns),
            Some(&0)
        );
    }
}
