//! Purpose: legacy merged config tests derived from `packages/opencode/test/config/config.test.ts`
//! (defaults, JSON/JSONC precedence, `{env:}`/`{file:}` substitution incl. secret-safe `$schema`
//! seeding (:680-742), invalid values, `.opencode` dirs and markdown agents/commands, plugin and
//! instructions merges and origins, managed settings (:1372-1422), project-config disable,
//! `OPENCODE_PERMISSION`, `OPENCODE_CONFIG_CONTENT`, legacy tools, plugin spec helpers).
//! Scope: integration tests, fixture-based, temp dirs only.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/legacy/load.rs
//! `-- config/src/legacy/plugin.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: every test isolates HOME, global config and managed dir under its own temp dir.
//! Contract tests: config/tests/legacy_load.rs

use opencode_config::Environment;
use opencode_config::legacy::{
    self, Instance, NoAccounts, Scope, Sources, SystemManagedPreferences, UnsupportedRemote,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Tree {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Environment,
    project: PathBuf,
}

impl Tree {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let mut env = Environment::isolated(root.join("home"));
        env.global_config = root.join("global");
        env.set_var(
            "OPENCODE_TEST_MANAGED_CONFIG_DIR",
            root.join("managed").display().to_string(),
        );
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        Self {
            _tmp: tmp,
            root,
            env,
            project,
        }
    }

    fn write(&self, path: &Path, value: &Value) {
        write(path, &value.to_string());
    }

    fn load(&self) -> Result<legacy::LegacyState, opencode_config::ConfigError> {
        let instance = Instance {
            directory: self.project.clone(),
            worktree: self.project.clone(),
        };
        let managed = SystemManagedPreferences {
            root: Some(self.root.join("mdm")),
        };
        let sources = Sources {
            accounts: &NoAccounts,
            remote: &UnsupportedRemote,
            managed: &managed,
        };
        legacy::load(&self.env, &instance, &sources)
    }

    fn config(&self) -> Value {
        self.load().expect("load").config
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn schema(value: Value) -> Value {
    let mut out = json!({ "$schema": "https://opencode.ai/config.json" });
    out.as_object_mut()
        .expect("obj")
        .extend(value.as_object().expect("obj").clone());
    out
}

#[test]
fn defaults_username_and_seeds_global_jsonc_schema() {
    let tree = Tree::new();
    assert_eq!(tree.config()["username"], json!("testuser"));
    let seeded =
        std::fs::read_to_string(tree.env.global_config.join("opencode.jsonc")).expect("seed");
    assert!(seeded.contains("\"$schema\": \"https://opencode.ai/config.json\""));
    let mut tree = Tree::new();
    tree.env.username = None;
    assert_eq!(tree.config()["username"], json!("user"));
}

#[test]
fn does_not_seed_global_config_when_config_dir_is_set() {
    let mut tree = Tree::new();
    let custom = tree.root.join("custom");
    tree.env
        .set_var("OPENCODE_CONFIG_DIR", custom.display().to_string());
    tree.load().expect("load");
    assert!(!tree.env.global_config.join("opencode.jsonc").exists());
    assert!(
        std::fs::read_to_string(custom.join(".gitignore"))
            .expect("gitignore")
            .contains("node_modules")
    );
}

#[test]
fn loads_json_jsonc_and_ignores_legacy_tui_keys() {
    let tree = Tree::new();
    tree.write(
        &tree.project.join("opencode.json"),
        &schema(json!({ "model": "test/model", "theme": "legacy", "tui": { "scroll_speed": 4 } })),
    );
    let config = tree.config();
    assert_eq!(config["model"], json!("test/model"));
    assert_eq!(config.get("theme"), None);
    assert_eq!(config.get("tui"), None);
    write(
        &tree.project.join("opencode.jsonc"),
        "{\n  // comment\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"model\": \"base\",\n  \"username\": \"base\"\n}",
    );
    let config = tree.config();
    assert_eq!(
        (config["model"].clone(), config["username"].clone()),
        (json!("base"), json!("base"))
    );
}

#[test]
fn substitutes_env_and_file_without_writing_secrets_back() {
    let mut tree = Tree::new();
    tree.env.set_var("PRESERVE_VAR", "secret_value");
    let file = tree.project.join("opencode.json");
    tree.write(&file, &json!({ "username": "{env:PRESERVE_VAR}" }));
    assert_eq!(tree.config()["username"], json!("secret_value"));
    let content = std::fs::read_to_string(&file).expect("read");
    assert!(
        content.contains("{env:PRESERVE_VAR}")
            && content.contains("$schema")
            && !content.contains("secret_value")
    );
    write(
        &tree.project.join("included.md"),
        "const out = await Bun.$`echo hi`\n",
    );
    tree.write(&file, &schema(json!({ "username": "{file:included.md}" })));
    assert_eq!(
        tree.config()["username"],
        json!("const out = await Bun.$`echo hi`")
    );
    std::fs::create_dir_all(tree.env.home.join("cfg")).expect("mkdir");
    write(&tree.env.home.join("cfg/name.txt"), "  home-user \n");
    tree.write(
        &file,
        &schema(json!({ "username": "{file:~/cfg/name.txt}" })),
    );
    assert_eq!(tree.config()["username"], json!("home-user"));
}

#[test]
fn rejects_invalid_values_and_json() {
    let tree = Tree::new();
    tree.write(
        &tree.project.join("opencode.json"),
        &schema(json!({ "model": 42 })),
    );
    assert_eq!(
        tree.load().expect_err("invalid").name(),
        "ConfigInvalidError"
    );
    write(&tree.project.join("opencode.json"), "{ invalid json }");
    assert_eq!(tree.load().expect_err("json").name(), "ConfigJsonError");
    tree.write(
        &tree.project.join("opencode.json"),
        &schema(json!({ "username": "{file:missing.txt}" })),
    );
    let error = tree.load().expect_err("missing file").to_named_json();
    assert!(
        error["data"]["message"]
            .as_str()
            .expect("msg")
            .starts_with("bad file reference: \"{file:missing.txt}\"")
    );
}

#[test]
fn loads_dot_opencode_config_agents_commands_and_modes() {
    let tree = Tree::new();
    let dot = tree.project.join(".opencode");
    tree.write(
        &dot.join("opencode.json"),
        &schema(json!({ "model": "dot/model" })),
    );
    write(
        &dot.join("agent/helper.md"),
        "---\nmodel: test/model\nmode: subagent\n---\nHelper subagent prompt",
    );
    write(
        &dot.join("agents/nested/deep.md"),
        "---\ndescription: Deep\n---\nDeep prompt",
    );
    write(
        &dot.join("command/hello.md"),
        "---\ndescription: Say hi\n---\nHello $ARGUMENTS",
    );
    write(&dot.join("commands/sub/x.md"), "Run x");
    write(
        &dot.join("mode/focus.md"),
        "---\ntemperature: 0.1\n---\nFocus",
    );
    let config = tree.config();
    assert_eq!(config["model"], json!("dot/model"));
    let helper = &config["agent"]["helper"];
    assert_eq!(
        (
            helper["name"].clone(),
            helper["mode"].clone(),
            helper["prompt"].clone()
        ),
        (
            json!("helper"),
            json!("subagent"),
            json!("Helper subagent prompt")
        )
    );
    assert_eq!(
        config["agent"]["nested/deep"]["prompt"],
        json!("Deep prompt")
    );
    assert_eq!(
        config["command"]["hello"]["template"],
        json!("Hello $ARGUMENTS")
    );
    assert_eq!(config["command"]["sub/x"]["template"], json!("Run x"));
    assert_eq!(config["agent"]["focus"]["mode"], json!("primary"));
    assert_eq!(config["agent"]["focus"]["temperature"], json!(0.1));
    assert!(
        std::fs::read_to_string(dot.join(".gitignore"))
            .expect("gitignore")
            .contains("bun.lock")
    );
}

fn tree_with(global: Value, project: Option<Value>, local: Option<Value>) -> Tree {
    let tree = Tree::new();
    tree.write(
        &tree.env.global_config.join("opencode.json"),
        &schema(global),
    );
    if let Some(project) = project {
        tree.write(&tree.project.join("opencode.json"), &schema(project));
    }
    if let Some(local) = local {
        tree.write(
            &tree.project.join(".opencode/opencode.json"),
            &schema(local),
        );
    }
    tree
}

#[test]
fn instructions_concatenate_and_dedupe_across_sources() {
    let tree = tree_with(
        json!({ "instructions": ["duplicate.md", "global-only.md"] }),
        None,
        Some(json!({ "instructions": ["duplicate.md", "local-only.md"] })),
    );
    assert_eq!(
        tree.config()["instructions"],
        json!(["duplicate.md", "global-only.md", "local-only.md"])
    );
}

#[test]
fn plugins_dedupe_with_origins_aligned() {
    let tree = tree_with(
        json!({ "plugin": [["shared-plugin@1.0.0", { "source": "global" }], "global-only@1.0.0", "duplicate-plugin"] }),
        None,
        Some(
            json!({ "plugin": [["shared-plugin@2.0.0", { "source": "local" }], "local-only@1.0.0", "duplicate-plugin"] }),
        ),
    );
    let state = tree.load().expect("load");
    let names: Vec<String> = state
        .plugin_origins
        .iter()
        .map(|o| legacy::plugin::specifier(&o.spec))
        .collect();
    assert_eq!(
        names,
        vec![
            "global-only@1.0.0",
            "shared-plugin@2.0.0",
            "local-only@1.0.0",
            "duplicate-plugin"
        ]
    );
    let specs: Vec<Value> = state
        .plugin_origins
        .iter()
        .map(|o| o.spec.clone())
        .collect();
    assert_eq!(state.config["plugin"], Value::Array(specs));
    let shared = state
        .plugin_origins
        .iter()
        .find(|o| legacy::plugin::specifier(&o.spec) == "shared-plugin@2.0.0");
    assert_eq!(shared.map(|o| o.scope), Some(Scope::Local));
}

#[test]
fn project_config_disable_keeps_global_scope() {
    let mut tree = tree_with(
        json!({ "model": "global/model", "plugin": ["global-plugin"] }),
        Some(json!({ "model": "project/model" })),
        Some(json!({ "model": "local/model" })),
    );
    tree.env.set_var("OPENCODE_DISABLE_PROJECT_CONFIG", "true");
    let state = tree.load().expect("load");
    assert_eq!(state.config["model"], json!("global/model"));
    assert_eq!(state.plugin_origins[0].scope, Scope::Global);
    assert!(
        !state
            .directories
            .iter()
            .any(|d| d.starts_with(&tree.project))
    );
}

#[test]
fn precedence_matrix_and_provenance() {
    let mut tree = tree_with(
        json!({ "model": "global", "username": "g" }),
        Some(json!({ "model": "project" })),
        Some(json!({ "model": "dot" })),
    );
    let custom = tree.root.join("custom.json");
    tree.write(
        &custom,
        &schema(json!({ "model": "custom", "share": "manual" })),
    );
    tree.env
        .set_var("OPENCODE_CONFIG", custom.display().to_string());
    tree.env.set_var(
        "OPENCODE_CONFIG_CONTENT",
        json!({ "model": "content", "small_model": "s/m" }).to_string(),
    );
    let state = tree.load().expect("load");
    assert_eq!(state.config["model"], json!("content"));
    assert_eq!(
        state.provenance.source(&["model"]),
        Some("OPENCODE_CONFIG_CONTENT")
    );
    assert_eq!(
        state.provenance.source(&["share"]),
        Some(custom.display().to_string().as_str())
    );
    assert_eq!(
        state.provenance.source(&["username"]),
        Some(tree.env.global_config.display().to_string().as_str())
    );
    tree.write(
        &tree.root.join("managed/opencode.json"),
        &json!({ "model": "managed/json" }),
    );
    tree.write(
        &tree.root.join("managed/opencode.jsonc"),
        &json!({ "model": "managed/jsonc", "share": "disabled" }),
    );
    let state = tree.load().expect("load");
    assert_eq!(
        (state.config["model"].clone(), state.config["share"].clone()),
        (json!("managed/jsonc"), json!("disabled"))
    );
    assert_eq!(
        state.config["username"],
        json!("g"),
        "managed keeps unrelated user values"
    );
}

#[test]
fn permission_env_tools_autoshare_and_compaction_flags() {
    let mut tree = Tree::new();
    tree.write(&tree.project.join("opencode.json"), &schema(json!({ "tools": { "write": false, "bash": true }, "permission": { "bash": "ask" }, "autoshare": true })));
    tree.env.set_var("OPENCODE_PERMISSION", "{ not json");
    tree.env.set_var("OPENCODE_DISABLE_AUTOCOMPACT", "1");
    tree.env.set_var("OPENCODE_DISABLE_PRUNE", "TRUE");
    let config = tree.config();
    assert_eq!(
        config["permission"],
        json!({ "edit": "deny", "bash": "ask" })
    );
    assert_eq!(config["share"], json!("auto"));
    assert_eq!(
        config["compaction"],
        json!({ "auto": false, "prune": false })
    );
    tree.env
        .set_var("OPENCODE_PERMISSION", json!({ "read": "deny" }).to_string());
    assert_eq!(tree.config()["permission"]["read"], json!("deny"));
}

#[test]
fn config_content_substitutes_env_and_file_tokens() {
    let mut tree = Tree::new();
    write(&tree.project.join("token.txt"), "from-file");
    tree.env.set_var("CONTENT_USER", "env-user");
    tree.env.set_var(
        "OPENCODE_CONFIG_CONTENT",
        json!({ "username": "{env:CONTENT_USER}", "model": "{file:token.txt}" }).to_string(),
    );
    let config = tree.config();
    assert_eq!(
        (config["username"].clone(), config["model"].clone()),
        (json!("env-user"), json!("from-file"))
    );
}

#[test]
fn resolves_relative_plugin_specs_against_declaring_file() {
    let tree = Tree::new();
    write(&tree.project.join("plugins/local.ts"), "export default {}");
    write(&tree.project.join("pdir/index.ts"), "export default {}");
    tree.write(
        &tree.project.join("opencode.json"),
        &schema(json!({ "plugin": ["./plugins/local.ts", "./pdir", "@scope/pkg@1.2.3"] })),
    );
    write(
        &tree.project.join(".opencode/plugin/auto.js"),
        "export default {}",
    );
    let state = tree.load().expect("load");
    let url = |p: &Path| legacy::plugin::file_url(p);
    assert_eq!(
        state.config["plugin"],
        json!([
            url(&tree.project.join("plugins/local.ts")),
            url(&tree.project.join("pdir/index.ts")),
            "@scope/pkg@1.2.3",
            url(&tree.project.join(".opencode/plugin/auto.js"))
        ])
    );
    assert_eq!(
        legacy::plugin::package_name("@scope/pkg@1.2.3"),
        "@scope/pkg"
    );
    assert_eq!(legacy::plugin::package_name("pkg"), "pkg");
}
