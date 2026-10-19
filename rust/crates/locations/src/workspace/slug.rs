//! Purpose: random `adjective-noun` names (port of packages/core/src/util/slug.ts `Slug.create`).
//! Scope: pure function over a fixed word list.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! `-- locations/src/workspace/worktree.rs
//! Invariants: word lists and order match the baseline exactly.
//! Contract tests: locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

const ADJECTIVES: &[&str] = &[
    "brave", "calm", "clever", "cosmic", "crisp", "curious", "eager", "gentle", "glowing", "happy",
    "hidden", "jolly", "kind", "lucky", "mighty", "misty", "neon", "nimble", "playful", "proud",
    "quick", "quiet", "shiny", "silent", "stellar", "sunny", "swift", "tidy", "witty",
];

const NOUNS: &[&str] = &[
    "cabin", "cactus", "canyon", "circuit", "comet", "eagle", "engine", "falcon", "forest",
    "garden", "harbor", "island", "knight", "lagoon", "meadow", "moon", "mountain", "nebula",
    "orchid", "otter", "panda", "pixel", "planet", "river", "rocket", "sailor", "squid", "star",
    "tiger", "wizard", "wolf",
];

/// `Slug.create()`.
pub fn create() -> String {
    format!(
        "{}-{}",
        ADJECTIVES[rand::random_range(0..ADJECTIVES.len())],
        NOUNS[rand::random_range(0..NOUNS.len())]
    )
}
