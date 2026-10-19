//! Purpose: current ordered-document configuration (`packages/core/src/config.ts:109-227`):
//! global directory documents, project files walked up to the project boundary, `.opencode`
//! supplemental directories, V1 auto-migration, `latest`, and policy statements.
//! Scope: Location snapshot loader (read once per Location open).
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/jsonc/mod.rs
//! |-- config/src/migrate.rs
//! `-- config/src/schema/mod.rs
//! External:
//! |-- fs::read_safe
//! |-- fs::up
//! |-- jsonc::parse
//! |-- migrate::is_v1
//! |-- migrate::migrate
//! |-- schema::decode
//! |-- schema::v1::info
//! |-- schema::v2::info
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- config/src/lib.rs
//! |-- config/src/provider.rs
//! |-- config/src/resources.rs
//! |-- config/tests/current.rs
//! |-- config/tests/legacy_update.rs
//! `-- config/tests/markdown.rs
//! Invariants: entries are lowest to highest priority: global documents, global directory,
//! direct project files (outermost first), then each `.opencode` (outermost first) documents and
//! directory. Invalid or unparseable files are skipped silently. Policies load in reverse.
//! Contract tests: config/tests/current.rs

use crate::env::Environment;
use crate::error::ConfigError;
use crate::{fs, jsonc, migrate, schema};
use serde_json::Value;
use std::path::{Path, PathBuf};

const NAMES: [&str; 2] = ["opencode.json", "opencode.jsonc"];

/// `Config.Document`: a decoded `Config.Info` plus the file it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub info: Value,
}

/// `Config.Entry`.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Document(Document),
    Directory(PathBuf),
}

impl Entry {
    pub fn document(&self) -> Option<&Document> {
        match self {
            Self::Document(document) => Some(document),
            Self::Directory(_) => None,
        }
    }

    pub fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(path) => Some(path),
            Self::Document(_) => None,
        }
    }
}

/// What the loader needs from the opened Location.
#[derive(Debug, Clone)]
pub struct LocationSnapshot {
    pub directory: PathBuf,
    pub project_directory: PathBuf,
}

/// `Config.Service` state for one Location.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentConfig {
    entries: Vec<Entry>,
}

impl CurrentConfig {
    /// Discover and decode configuration for `location` (the `layer` body in config.ts).
    pub fn load(env: &Environment, location: &LocationSnapshot) -> Result<Self, ConfigError> {
        let global = env.current_global_config();
        let location_is_global = location.directory == global;
        let mut targets = vec![".opencode"];
        targets.extend(NAMES.iter().rev());
        let discovered = if location_is_global {
            Vec::new()
        } else {
            fs::up(
                &targets,
                &location.directory,
                Some(&location.project_directory),
            )
        };
        let is_dot = |path: &PathBuf| path.file_name().is_some_and(|name| name == ".opencode");
        let mut directories = vec![global];
        directories.extend(discovered.iter().filter(|path| is_dot(path)).rev().cloned());
        let mut direct = Vec::new();
        for path in discovered.iter().filter(|path| !is_dot(path)).rev() {
            direct.extend(load_file(path)?);
        }
        let mut supplementary = Vec::new();
        for directory in &directories {
            supplementary.push(load_directory(directory)?);
        }
        let mut entries: Vec<Entry> = supplementary.first().cloned().unwrap_or_default();
        entries.extend(direct.into_iter().map(Entry::Document));
        entries.extend(supplementary.into_iter().skip(1).flatten());
        Ok(Self { entries })
    }

    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    /// Entries from lowest to highest priority.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.entries.iter().filter_map(Entry::document)
    }

    /// `Config.latest(entries, key)`.
    pub fn latest(&self, key: &str) -> Option<&Value> {
        latest(&self.entries, key)
    }

    /// Policy statements in evaluation order: documents reversed, statement order kept.
    pub fn policies(&self) -> Vec<Value> {
        let documents: Vec<&Document> = self.documents().collect();
        documents
            .iter()
            .rev()
            .flat_map(|doc| {
                doc.info
                    .pointer("/experimental/policies")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    }

    /// `Policy.evaluate`: the last statement matching action and resource wins.
    pub fn evaluate_policy(&self, action: &str, resource: &str, fallback: &str) -> String {
        self.policies()
            .iter()
            .rev()
            .find(|statement| {
                let get = |key: &str| {
                    statement
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                };
                wildcard(action, get("action")) && wildcard(resource, get("resource"))
            })
            .and_then(|statement| statement.get("effect").and_then(Value::as_str))
            .unwrap_or(fallback)
            .to_string()
    }
}

/// `Config.latest` over any entry slice.
pub fn latest<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Value> {
    entries
        .iter()
        .filter_map(Entry::document)
        .rev()
        .find_map(|doc| doc.info.get(key))
}

fn load_directory(directory: &Path) -> Result<Vec<Entry>, ConfigError> {
    let mut out = Vec::new();
    for name in NAMES {
        out.extend(load_file(&directory.join(name))?.map(Entry::Document));
    }
    out.push(Entry::Directory(directory.to_path_buf()));
    Ok(out)
}

/// `loadFile`: unreadable, empty, unparseable or invalid files yield `None`.
pub fn load_file(path: &Path) -> Result<Option<Document>, ConfigError> {
    let Some(text) = fs::read_safe(path)? else {
        return Ok(None);
    };
    if text.is_empty() {
        return Ok(None);
    }
    let (Some(input), errors) = jsonc::parse(&text) else {
        return Ok(None);
    };
    if !errors.is_empty() {
        return Ok(None);
    }
    Ok(decode_info(&input).map(|info| Document {
        path: Some(path.to_path_buf()),
        info,
    }))
}

/// Decode raw JSON as `Config.Info`, migrating V1 documents first.
pub fn decode_info(input: &Value) -> Option<Value> {
    if migrate::is_v1(input) {
        let v1 = schema::decode(schema::v1::info(), input).ok()?;
        return schema::decode(schema::v2::info(), &migrate::migrate(&v1)).ok();
    }
    schema::decode(schema::v2::info(), input).ok()
}

/// `Wildcard.match` (`*` any run, `?` one char, trailing ` *` also matches the bare prefix).
pub fn wildcard(input: &str, pattern: &str) -> bool {
    let input: Vec<char> = input.replace('\\', "/").chars().collect();
    let pattern = pattern.replace('\\', "/");
    if let Some(prefix) = pattern.strip_suffix(" *") {
        let prefix: Vec<char> = prefix.chars().collect();
        if glob_match(&input, &prefix) {
            return true;
        }
    }
    glob_match(&input, &pattern.chars().collect::<Vec<_>>())
}

fn glob_match(input: &[char], pattern: &[char]) -> bool {
    let (mut i, mut p) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while i < input.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == input[i]) && pattern[p] != '*' {
            i += 1;
            p += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = i;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|ch| *ch == '*')
}
