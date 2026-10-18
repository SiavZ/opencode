//! Purpose: small baseline helpers used by `SessionV2.create`: `Slug.create`, `Date#toISOString`
//! and node `path.relative` for absolute POSIX paths.
//! Scope: pure functions.
//! Uses:
//! `-- none
//! External:
//! |-- jiff::Timestamp
//! `-- rand::random_range
//! Used by:
//! rust/crates/
//! `-- session-runtime/src/service.rs
//! Invariants: word lists equal packages/core/src/util/slug.ts; ISO strings have millisecond precision.
//! Contract tests: session-runtime/src/service/util.rs

use std::path::{Component, Path};

const ADJECTIVES: [&str; 29] = [
    "brave", "calm", "clever", "cosmic", "crisp", "curious", "eager", "gentle", "glowing", "happy",
    "hidden", "jolly", "kind", "lucky", "mighty", "misty", "neon", "nimble", "playful", "proud",
    "quick", "quiet", "shiny", "silent", "stellar", "sunny", "swift", "tidy", "witty",
];
const NOUNS: [&str; 31] = [
    "cabin", "cactus", "canyon", "circuit", "comet", "eagle", "engine", "falcon", "forest",
    "garden", "harbor", "island", "knight", "lagoon", "meadow", "moon", "mountain", "nebula",
    "orchid", "otter", "panda", "pixel", "planet", "river", "rocket", "sailor", "squid", "star",
    "tiger", "wizard", "wolf",
];

/// `Slug.create()`: `<adjective>-<noun>`.
pub fn slug() -> String {
    format!(
        "{}-{}",
        ADJECTIVES[rand::random_range(0..ADJECTIVES.len())],
        NOUNS[rand::random_range(0..NOUNS.len())]
    )
}

/// `new Date(millis).toISOString()`.
pub fn iso(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis)
        .map(|timestamp| timestamp.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

/// `path.relative(from, to).replaceAll("\\", "/")` for absolute paths.
pub fn relative(from: &str, to: &str) -> String {
    let normal = |path: &str| -> Vec<String> {
        Path::new(path)
            .components()
            .fold(Vec::new(), |mut parts, component| {
                match component {
                    Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
                    Component::ParentDir => {
                        parts.pop();
                    }
                    _ => {}
                }
                parts
            })
    };
    let from = normal(from);
    let to = normal(to);
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
    parts.extend(to[common..].iter().cloned());
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_node() {
        // fixture: values from node `path.relative` and `Date.prototype.toISOString`.
        assert_eq!(relative("/repo", "/repo/packages/core"), "packages/core");
        assert_eq!(relative("/repo", "/repo"), "");
        assert_eq!(relative("/repo/a", "/repo/b/c"), "../b/c");
        assert_eq!(relative("/", "/tmp/x"), "tmp/x");
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_791_051_437_319), "2026-10-03T18:17:17.319Z");
        let slug = slug();
        let (adjective, noun) = slug.split_once('-').unwrap();
        assert!(ADJECTIVES.contains(&adjective) && NOUNS.contains(&noun));
    }
}
