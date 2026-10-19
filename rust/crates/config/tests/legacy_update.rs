//! Purpose: secret-safe write tests derived from `packages/opencode/test/config/config.test.ts`
//! (:360-445 empty shell sentinel/removal, diagnostics never expose values; :489-526 native
//! permissions are rejected without writing; JSONC comments preserved) plus provider/resource
//! consumer checks for the current config.
//! Scope: integration tests, fixture-based, temp dirs only.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/mod.rs
//! |-- config/src/lib.rs
//! `-- config/src/resources.rs
//! External:
//! |-- legacy::update
//! |-- legacy::update_global
//! |-- pretty_assertions::assert_eq
//! |-- resources::agents
//! |-- resources::plugins
//! |-- resources::references
//! |-- resources::skills
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: failed updates leave file bytes unchanged; errors and diagnostics never contain
//! config values.
//! Contract tests: config/tests/legacy_update.rs

use opencode_config::current::{Document, Entry};
use opencode_config::legacy::{self, DiagnosticKind};
use opencode_config::{Environment, resources};
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;

fn global_env(root: &Path) -> Environment {
    let env = Environment::isolated(root);
    std::fs::create_dir_all(&env.global_config).expect("mkdir");
    env
}

#[test]
fn project_update_keeps_empty_shell_sentinel_and_env_tokens() {
    let tmp = tempfile::tempdir().expect("tmp");
    let env = Environment::isolated(tmp.path().join("home"));
    let file = tmp.path().join("config.json");
    std::fs::write(&file, json!({ "$schema": "https://opencode.ai/config.json", "shell": "bash", "username": "{env:SECRET}" }).to_string()).expect("w");
    let mut env = env;
    env.set_var("SECRET", "expanded-secret");
    legacy::update(tmp.path(), &json!({ "shell": "" }), &env).expect("update");
    let written = std::fs::read_to_string(&file).expect("read");
    assert!(
        written.contains("\"shell\": \"\"")
            && written.contains("{env:SECRET}")
            && !written.contains("expanded-secret")
    );
}

#[test]
fn global_update_removes_empty_shell_in_json_and_jsonc() {
    let tmp = tempfile::tempdir().expect("tmp");
    let env = global_env(tmp.path());
    let json_file = env.global_config.join("opencode.json");
    std::fs::write(
        &json_file,
        json!({ "$schema": "https://opencode.ai/config.json", "shell": "bash" }).to_string(),
    )
    .expect("w");
    legacy::update_global(&env, &json!({ "shell": "" })).expect("update");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_file).expect("r")).expect("json");
    assert_eq!(written.get("shell"), None);
    std::fs::remove_file(&json_file).expect("rm");
    let jsonc = env.global_config.join("opencode.jsonc");
    std::fs::write(&jsonc, "{\n  \"model\": \"test/model\",\n  \"shell\": \"bash\",\n  // keep me\n  \"username\": \"{env:SECRET}\"\n}").expect("w");
    let updated = legacy::update_global(&env, &json!({ "shell": "" })).expect("update");
    let written = std::fs::read_to_string(&jsonc).expect("r");
    assert!(
        !written.contains("\"shell\"")
            && written.contains("// keep me")
            && written.contains("{env:SECRET}")
    );
    assert_eq!(
        (updated.info.get("shell"), updated.info["model"].clone()),
        (None, json!("test/model"))
    );
}

#[test]
fn global_update_diagnostics_do_not_expose_values() {
    let tmp = tempfile::tempdir().expect("tmp");
    let env = global_env(tmp.path());
    let file = env.global_config.join("opencode.json");
    std::fs::write(&file, json!({ "$schema": "https://opencode.ai/config.json", "providers": { "example": { "settings": { "apiKey": "keep-me" } } } }).to_string()).expect("w");
    let updated = legacy::update_global(&env, &json!({ "username": "updated" })).expect("update");
    assert_eq!(updated.diagnostics.0.len(), 1);
    let diag = &updated.diagnostics.0[0];
    assert_eq!(
        (
            diag.source.as_str(),
            diag.diagnostic.kind,
            diag.diagnostic.path.clone()
        ),
        (
            file.to_str().expect("p"),
            DiagnosticKind::Unsupported,
            vec!["providers".to_string()]
        )
    );
    assert!(!format!("{:?}", updated.diagnostics).contains("keep-me"));
    assert!(
        std::fs::read_to_string(&file)
            .expect("r")
            .contains("keep-me"),
        "native settings preserved on disk"
    );
}

#[test]
fn native_permissions_are_rejected_without_writing() {
    for name in ["opencode.json", "opencode.jsonc"] {
        let tmp = tempfile::tempdir().expect("tmp");
        let env = global_env(tmp.path());
        let file = env.global_config.join(name);
        let before = json!({ "$schema": "https://opencode.ai/config.json", "permissions": [{ "action": "read", "resource": "secret-resource", "effect": "deny" }] }).to_string();
        std::fs::write(&file, &before).expect("w");
        let error =
            legacy::update_global(&env, &json!({ "username": "changed" })).expect_err("reject");
        let named = error.to_named_json();
        assert_eq!(
            (
                named["data"]["path"].clone(),
                named["data"]["issues"][0]["path"].clone()
            ),
            (json!(file.display().to_string()), json!(["permissions"]))
        );
        assert!(!named.to_string().contains("secret-resource"));
        assert_eq!(std::fs::read_to_string(&file).expect("r"), before);
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let file = tmp.path().join("config.json");
    let before = json!({ "agents": { "reviewer": { "permissions": [] } } }).to_string();
    std::fs::write(&file, &before).expect("w");
    let error = legacy::update(
        tmp.path(),
        &json!({ "username": "changed" }),
        &Environment::isolated(tmp.path().join("h")),
    )
    .expect_err("reject");
    assert_eq!(
        error.to_named_json()["data"]["issues"][0]["path"],
        json!(["agents", "reviewer", "permissions"])
    );
    assert_eq!(std::fs::read_to_string(&file).expect("r"), before);
}

#[test]
fn resource_consumers_resolve_references_skills_plugins_and_permissions() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (home, location) = (tmp.path().join("home"), tmp.path().join("loc"));
    let dot = location.join(".opencode");
    std::fs::create_dir_all(dot.join("agent")).expect("mkdir");
    std::fs::write(
        dot.join("agent/rev.md"),
        "---\ntemperature: 0.5\n---\nReview",
    )
    .expect("w");
    std::fs::create_dir_all(dot.join("plugins")).expect("mkdir");
    std::fs::write(dot.join("plugins/p.ts"), "").expect("w");
    let doc = Document {
        path: Some(location.join("opencode.json")),
        info: json!({
            "default_agent": "rev",
            "permissions": [{ "action": "read", "resource": "~/secret/**", "effect": "deny" }, { "action": "bash", "resource": "$HOME/x", "effect": "deny" }],
            "skills": ["./skills", "~/shared", "https://example.com/skills/"],
            "references": { "local": { "path": "../library" }, "sdk": { "repository": "github.com/example/sdk", "branch": "main" }, "short": "github.com/x/docs", "bad alias": "./x" },
            "plugins": ["pkg", { "package": "./local.ts", "options": { "a": 1 } }],
        }),
    };
    let entries = vec![Entry::Document(doc), Entry::Directory(dot.clone())];
    let agents = resources::agents(&entries, &home);
    assert_eq!(agents.default_agent.as_deref(), Some("rev"));
    assert_eq!(
        agents.permissions[0]["resource"],
        json!(format!("{}/secret/**", home.display()))
    );
    assert_eq!(
        agents.permissions[1]["resource"],
        json!("$HOME/x"),
        "bash resources are not expanded"
    );
    assert_eq!(
        agents.agents[0],
        (
            "rev".to_string(),
            json!({ "request": { "body": { "temperature": 0.5 } }, "system": "Review" })
        )
    );
    let skills = resources::skills(&entries, &home, &location);
    assert_eq!(skills.len(), 5);
    assert_eq!(
        skills[3],
        resources::SkillSource::Directory(home.join("shared"))
    );
    let refs = resources::references(&entries, &home, &location);
    assert_eq!(
        refs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["local", "sdk", "short"]
    );
    assert_eq!(
        refs[0].1,
        json!({ "type": "local", "path": tmp.path().join("library").display().to_string() })
    );
    assert_eq!(
        refs[2].1,
        json!({ "type": "git", "repository": "github.com/x/docs" })
    );
    let plugins = resources::plugins(&entries, &location);
    assert_eq!(
        plugins[1],
        (
            location.join("local.ts").display().to_string(),
            Some(json!({ "a": 1 }))
        )
    );
    assert_eq!(plugins[2].0, dot.join("plugins/p.ts").display().to_string());
}
