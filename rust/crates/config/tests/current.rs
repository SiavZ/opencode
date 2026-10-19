//! Purpose: tests for the current ordered-document Config derived from
//! `packages/core/test/config/config.test.ts` (:53-145 latest/isV1/migrate, :146-265 discovery,
//! :266-668 resources and V1 migration, :669-798 invalid files, policies, project boundary).
//! Scope: integration tests, fixture-based, temp dirs only.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/lib.rs
//! `-- config/src/migrate.rs
//! External:
//! |-- migrate::migrate
//! |-- pretty_assertions::assert_eq
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches the real HOME; each test owns its temp tree.
//! Contract tests: config/tests/current.rs

use opencode_config::current::{CurrentConfig, Document, Entry, LocationSnapshot, latest};
use opencode_config::{Environment, migrate};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn load(dir: &Path, global: &Path, project: &Path) -> CurrentConfig {
    let mut env = Environment::isolated(dir.join("home"));
    env.global_config = global.to_path_buf();
    let location = LocationSnapshot {
        directory: dir.to_path_buf(),
        project_directory: project.to_path_buf(),
    };
    CurrentConfig::load(&env, &location).expect("load")
}

fn schemas(config: &CurrentConfig) -> Vec<Value> {
    config
        .documents()
        .map(|doc| doc.info.get("$schema").cloned().unwrap_or(Value::Null))
        .collect()
}

#[test]
fn latest_returns_last_defined_scalar() {
    let doc = |info: Value| Entry::Document(Document { path: None, info });
    let entries = vec![
        doc(json!({ "model": "openrouter/openai/gpt-5" })),
        Entry::Directory(PathBuf::from("/skills")),
        doc(json!({})),
        doc(json!({ "model": "openrouter/openai/gpt-5.5" })),
    ];
    assert_eq!(
        latest(&entries, "model"),
        Some(&json!("openrouter/openai/gpt-5.5"))
    );
    assert_eq!(latest(&entries, "default_agent"), None);
}

#[test]
fn detects_v1_from_any_v1_only_key() {
    assert!(migrate::is_v1(&json!({ "snapshot": false })));
    assert!(migrate::is_v1(&json!({ "snapshot": false, "agents": {} })));
    assert!(migrate::is_v1(&json!({ "reference": {} })));
    assert!(!migrate::is_v1(
        &json!({ "shell": "/bin/zsh", "model": "anthropic/claude" })
    ));
    assert!(!migrate::is_v1(&json!({ "references": {} })));
}

#[test]
fn migrates_v1_provider_setup_options_and_commands() {
    let migrated = migrate::migrate(&json!({ "provider": { "bedrock": {
        "npm": "@ai-sdk/amazon-bedrock",
        "options": { "headers": { "x-test": "1" }, "body": { "trace": true }, "region": "us-east-1", "profile": "dev" },
    } } }));
    assert_eq!(
        migrated["providers"]["bedrock"]["api"],
        json!({ "type": "aisdk", "package": "@ai-sdk/amazon-bedrock", "settings": { "region": "us-east-1", "profile": "dev" } })
    );
    assert_eq!(
        migrated["providers"]["bedrock"]["request"],
        json!({ "headers": { "x-test": "1" }, "body": { "trace": true } })
    );
    let command = json!({ "template": "Review changes", "description": "Review code", "agent": "reviewer",
        "model": "anthropic/claude", "variant": "high", "subtask": true });
    assert_eq!(
        migrate::migrate(&json!({ "command": { "review": command.clone() } }))["commands"],
        json!({ "review": command })
    );
}

#[test]
fn empty_when_no_files_exist() {
    let tmp = tempfile::tempdir().expect("tmp");
    let global = tmp.path().join("global");
    let config = load(tmp.path(), &global, tmp.path());
    assert_eq!(config.entries(), &[Entry::Directory(global)]);
}

#[test]
fn loads_json_then_jsonc_and_snapshots_on_open() {
    let tmp = tempfile::tempdir().expect("tmp");
    let provider =
        r#"{"api":{"type":"native","settings":{}},"request":{"headers":{},"body":{}},"models":{}}"#;
    write(
        &tmp.path().join("opencode.json"),
        &format!(r#"{{"$schema":"base","providers":{{"base":{provider}}}}}"#),
    );
    write(
        &tmp.path().join("opencode.jsonc"),
        &format!(
            "{{\n  // Later global files override scalar fields while retaining providers.\n  \"$schema\": \"last\",\n  \"providers\": {{ \"last\": {provider} }},\n}}"
        ),
    );
    let config = load(tmp.path(), &tmp.path().join("global"), tmp.path());
    assert_eq!(schemas(&config), vec![json!("base"), json!("last")]);
    let first = config.documents().next().expect("doc");
    assert_eq!(
        first.path.as_deref(),
        Some(tmp.path().join("opencode.json").as_path())
    );
    assert!(config.documents().nth(1).expect("doc").info["providers"]["last"].is_object());
    write(
        &tmp.path().join("opencode.jsonc"),
        r#"{"$schema":"changed"}"#,
    );
    assert_eq!(
        schemas(&config),
        vec![json!("base"), json!("last")],
        "entries are a snapshot"
    );
}

#[test]
fn ignores_legacy_config_json_and_keeps_files_unwritten() {
    let tmp = tempfile::tempdir().expect("tmp");
    write(&tmp.path().join("config.json"), r#"{"$schema":"legacy"}"#);
    assert_eq!(
        load(tmp.path(), &tmp.path().join("global"), tmp.path())
            .documents()
            .count(),
        0
    );
    let contents = json!({ "shell": "/bin/zsh",
        "experimental": { "policies": [{ "effect": "deny", "action": "provider.use", "resource": "openai" }] } })
    .to_string();
    write(&tmp.path().join("opencode.json"), &contents);
    let config = load(tmp.path(), &tmp.path().join("global"), tmp.path());
    let doc = config.documents().next().expect("doc");
    assert_eq!(doc.info.get("$schema"), None);
    assert_eq!(doc.info["shell"], json!("/bin/zsh"));
    assert_eq!(
        doc.info["experimental"]["policies"][0],
        json!({ "effect": "deny", "action": "provider.use", "resource": "openai" })
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("opencode.json")).expect("read"),
        contents
    );
}

#[test]
fn ignores_invalid_file_and_keeps_valid_values() {
    let tmp = tempfile::tempdir().expect("tmp");
    write(&tmp.path().join("opencode.json"), r#"{"$schema":"base"}"#);
    write(&tmp.path().join("opencode.jsonc"), "{ invalid");
    assert_eq!(
        schemas(&load(tmp.path(), &tmp.path().join("global"), tmp.path())),
        vec![json!("base")]
    );
}

#[test]
fn policies_load_in_reverse_config_order() {
    let tmp = tempfile::tempdir().expect("tmp");
    let global = tmp.path().join("global");
    let policy = |effect: &str| json!({ "experimental": { "policies": [{ "effect": effect, "action": "provider.use", "resource": "openai" }] } });
    write(&global.join("opencode.json"), &policy("deny").to_string());
    write(
        &tmp.path().join("opencode.json"),
        &policy("allow").to_string(),
    );
    assert_eq!(
        load(tmp.path(), &global, tmp.path()).evaluate_policy("provider.use", "openai", "allow"),
        "deny"
    );
}

#[test]
fn loads_global_ancestor_and_dot_opencode_up_to_project_boundary() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (global, root) = (tmp.path().join("global"), tmp.path().join("repo"));
    let (parent, directory) = (root.join("packages"), root.join("packages/app"));
    let schema = |v: &str| json!({ "$schema": v }).to_string();
    write(&tmp.path().join("opencode.json"), &schema("outside"));
    write(&global.join("opencode.json"), &schema("global"));
    write(&root.join("opencode.json"), &schema("root"));
    write(&parent.join("opencode.jsonc"), &schema("parent"));
    write(&directory.join("opencode.json"), &schema("directory"));
    write(&root.join(".opencode/opencode.json"), &schema("root-dot"));
    write(
        &directory.join(".opencode/opencode.jsonc"),
        &schema("directory-dot"),
    );
    let mut env = Environment::isolated(tmp.path().join("home"));
    env.global_config = global.clone();
    let location = LocationSnapshot {
        directory: directory.clone(),
        project_directory: root.clone(),
    };
    let config = CurrentConfig::load(&env, &location).expect("load");
    let order: Vec<Value> = config
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Document(doc) => doc.info["$schema"].clone(),
            Entry::Directory(path) => json!(path.display().to_string()),
        })
        .collect();
    let dir = |p: &Path| json!(p.display().to_string());
    assert_eq!(
        order,
        vec![
            json!("global"),
            dir(&global),
            json!("root"),
            json!("parent"),
            json!("directory"),
            json!("root-dot"),
            dir(&root.join(".opencode")),
            json!("directory-dot"),
            dir(&directory.join(".opencode"))
        ]
    );
}

#[test]
fn global_location_skips_project_discovery() {
    let tmp = tempfile::tempdir().expect("tmp");
    let global = tmp.path().join("global");
    write(&global.join("opencode.json"), r#"{"$schema":"global"}"#);
    let config = load(&global, &global, &global);
    assert_eq!(schemas(&config), vec![json!("global")]);
}

#[test]
fn opencode_config_dir_replaces_current_global_directory() {
    let tmp = tempfile::tempdir().expect("tmp");
    let custom = tmp.path().join("custom");
    write(&custom.join("opencode.json"), r#"{"$schema":"custom"}"#);
    let mut env = Environment::isolated(tmp.path().join("home"));
    env.set_var("OPENCODE_CONFIG_DIR", custom.display().to_string());
    let location = LocationSnapshot {
        directory: tmp.path().join("p"),
        project_directory: tmp.path().join("p"),
    };
    let config = CurrentConfig::load(&env, &location).expect("load");
    assert_eq!(schemas(&config), vec![json!("custom")]);
    assert_eq!(config.entries().last(), Some(&Entry::Directory(custom)));
}

#[test]
fn loads_resources_and_migrates_v1_documents() {
    let tmp = tempfile::tempdir().expect("tmp");
    write(
        &tmp.path().join("opencode.json"),
        include_str!("fixtures/current-v1.json"),
    );
    let config = load(tmp.path(), &tmp.path().join("global"), tmp.path());
    let info = &config.documents().next().expect("doc").info;
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/current-v1-expected.json")).expect("fixture");
    assert_eq!(info, &expected);
}

#[test]
fn deprecated_reference_key_migrates_into_references() {
    let tmp = tempfile::tempdir().expect("tmp");
    let references = json!({ "local": { "path": "../library" },
        "sdk": { "repository": "github.com/example/sdk", "branch": "main" }, "shorthand": "github.com/example/docs" });
    write(
        &tmp.path().join("opencode.json"),
        &json!({ "reference": references }).to_string(),
    );
    let config = load(tmp.path(), &tmp.path().join("global"), tmp.path());
    assert_eq!(
        config.documents().next().expect("doc").info["references"],
        references
    );
}
