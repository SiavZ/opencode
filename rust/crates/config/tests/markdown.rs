//! Purpose: frontmatter, entry-name, substitution and provider-plugin tests derived from
//! `packages/opencode/test/config/markdown.test.ts` (fixtures copied verbatim),
//! `entry-name.test.ts`, `variable.ts` behavior, and `packages/core/src/config/plugin/provider.ts`.
//! Scope: integration tests, fixture-based.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/lib.rs
//! |-- config/src/provider.rs
//! |-- config/src/variable.rs
//! |-- schema/src/lib.rs
//! `-- schema/src/provider.rs
//! External:
//! |-- pretty_assertions::assert_eq
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture markdown files are byte-identical copies of the TS fixtures.
//! Contract tests: config/tests/markdown.rs

use opencode_config::current::{Document, Entry};
use opencode_config::legacy::markdown::{entry_name, parse};
use opencode_config::provider::{CatalogDraft, apply_catalog};
use opencode_config::variable::{Missing, Source, substitute};
use opencode_config::{ConfigError, Environment};
use opencode_schema::provider::{ModelInfo, ProviderInfo};
use opencode_schema::{ModelID, ProviderID};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .expect("fixture")
}

#[test]
fn frontmatter_with_unquoted_colons_falls_back_to_sanitized_yaml() {
    let parsed = parse(&fixture("frontmatter.md")).expect("parse");
    let get = |key: &str| parsed.data.get(key).cloned().unwrap_or(Value::Null);
    assert_eq!(
        get("description"),
        json!("This is a description wrapped in quotes")
    );
    assert_eq!(
        get("occupation"),
        json!("This man has the following occupation: Software Engineer")
    );
    assert_eq!(get("title"), json!("Hello World"));
    assert_eq!(get("name"), json!("John \"Doe\""));
    assert_eq!(get("family"), json!("He has no 'family'"));
    assert_eq!(get("summary"), json!("This is a summary\n"));
    assert_eq!(parsed.data.get("field"), None);
    assert_eq!(
        get("url"),
        json!("https://example.com:8080/path?query=value")
    );
    assert_eq!(get("time"), json!("The time is 12:30:00 PM"));
    assert_eq!(get("nested"), json!("First: Second: Third: Fourth"));
    assert_eq!(
        get("quoted_colon"),
        json!("Already quoted: no change needed")
    );
    assert_eq!(
        get("single_quoted_colon"),
        json!("Single quoted: also fine")
    );
    assert_eq!(
        get("mixed"),
        json!("He said \"hello: world\" and then left")
    );
    assert_eq!(parsed.data.get("empty"), Some(&Value::Null));
    assert_eq!(get("dollar"), json!("Use $' and $& for special patterns"));
    assert_eq!(parsed.data.get("fake_field"), None);
    assert!(parsed.content.contains("fake_field: this is not yaml"));
}

#[test]
fn empty_missing_and_header_only_frontmatter() {
    for name in ["empty-frontmatter.md", "no-frontmatter.md"] {
        let parsed = parse(&fixture(name)).expect("parse");
        assert!(parsed.data.is_empty(), "{name}");
        assert_eq!(parsed.content.trim(), "Content", "{name}");
    }
    let header = parse(&fixture("markdown-header.md")).expect("parse");
    assert!(header.data.is_empty());
    assert!(
        header
            .content
            .trim()
            .starts_with("# Response Formatting Requirements")
    );
    let weird = parse(&fixture("weird-model-id.md")).expect("parse");
    assert_eq!(
        weird.data["description"],
        json!("General coding and planning agent")
    );
    assert_eq!(weird.data["model"], json!("synthetic/hf:zai-org/GLM-4.7"));
    assert_eq!(
        weird.data["tools"],
        json!({ "write": true, "read": true, "edit": true })
    );
}

#[test]
fn entry_names_strip_only_anchored_prefixes() {
    assert_eq!(entry_name("agent/foo.md", &["agent/", "agents/"]), "foo");
    assert_eq!(
        entry_name("agents/nested/foo.md", &["agent/", "agents/"]),
        "nested/foo"
    );
    assert_eq!(
        entry_name("other/agent/foo.md", &["agent/", "agents/"]),
        "foo"
    );
    assert_eq!(
        entry_name("agents\\win\\x.md", &["agent/", "agents/"]),
        "win/x"
    );
}

#[test]
fn substitution_skips_commented_lines_and_escapes_file_content() {
    let tmp = tempfile::tempdir().expect("tmp");
    std::fs::write(tmp.path().join("q.txt"), "line \"one\"\nline two\n").expect("w");
    let mut env = Environment::isolated(tmp.path());
    env.set_var("EMPTY", "");
    let source = Source::Path(tmp.path().join("opencode.jsonc"));
    let text = "{\n  // \"x\": \"{file:missing.txt}\",\n  \"a\": \"{file:q.txt}\", \"b\": \"{env:EMPTY}{env:NOPE}\"\n}";
    let out = substitute(text, &source, &env, None, Missing::Error).expect("sub");
    assert_eq!(
        out,
        "{\n  // \"x\": \"{file:missing.txt}\",\n  \"a\": \"line \\\"one\\\"\\nline two\", \"b\": \"\"\n}"
    );
    let overlay = BTreeMap::from([("EMPTY".to_string(), "over".to_string())]);
    assert_eq!(
        substitute("{env:EMPTY}", &source, &env, Some(&overlay), Missing::Error).expect("sub"),
        "over"
    );
    assert_eq!(
        substitute("\"{file:nope}\"", &source, &env, None, Missing::Empty).expect("sub"),
        "\"\""
    );
    let error =
        substitute("\"{file:nope}\"", &source, &env, None, Missing::Error).expect_err("missing");
    assert!(matches!(error, ConfigError::Invalid { .. }));
}

#[derive(Default)]
struct Catalog {
    default: Option<(String, String)>,
    providers: BTreeMap<String, ProviderInfo>,
    models: BTreeMap<(String, String), ModelInfo>,
}

impl CatalogDraft for Catalog {
    fn set_default(&mut self, provider: &str, model: &str) {
        self.default = Some((provider.into(), model.into()));
    }
    fn update_provider(&mut self, id: &str, edit: &mut dyn FnMut(&mut ProviderInfo)) {
        edit(
            self.providers
                .entry(id.into())
                .or_insert_with(|| ProviderInfo::empty(ProviderID::new(id))),
        );
    }
    fn update_model(&mut self, provider: &str, model: &str, edit: &mut dyn FnMut(&mut ModelInfo)) {
        let entry = self.models.entry((provider.into(), model.into()));
        edit(
            entry.or_insert_with(|| {
                ModelInfo::empty(ProviderID::new(provider), ModelID::new(model))
            }),
        );
    }
}

#[test]
fn provider_plugin_applies_documents_in_priority_order() {
    let doc = |info: Value| Entry::Document(Document { path: None, info });
    let entries = vec![
        doc(
            json!({ "model": "openai/gpt-5", "providers": { "openai": { "name": "Base", "request": { "headers": { "a": "1" } },
            "models": { "gpt-5": { "request": { "headers": { "x": "1" } }, "variants": [{ "id": "fast", "body": { "k": 1 } }] } } } } }),
        ),
        doc(
            json!({ "model": "openai/gpt-5/mini", "providers": { "openai": { "name": "Override", "request": { "headers": { "b": "2" } },
            "models": { "gpt-5": { "name": "GPT", "disabled": true, "cost": { "input": 1, "output": 2 }, "limit": { "context": 100 },
                "variants": [{ "id": "fast", "headers": { "h": "v" } }] } } } } }),
        ),
    ];
    let mut catalog = Catalog::default();
    apply_catalog(&entries, &mut catalog);
    assert_eq!(
        catalog.default,
        Some(("openai".into(), "gpt-5/mini".into()))
    );
    let provider = serde_json::to_value(&catalog.providers["openai"]).expect("json");
    assert_eq!(
        (
            provider["name"].clone(),
            provider["request"]["headers"].clone()
        ),
        (json!("Override"), json!({ "a": "1", "b": "2" }))
    );
    let model = serde_json::to_value(&catalog.models[&("openai".to_string(), "gpt-5".to_string())])
        .expect("json");
    assert_eq!(model["name"], json!("GPT"));
    assert_eq!(model["enabled"], json!(false));
    assert_eq!(
        model["cost"],
        json!([{ "input": 1, "output": 2, "cache": { "read": 0, "write": 0 } }])
    );
    assert_eq!(model["limit"]["context"], json!(100));
    assert_eq!(
        model["variants"],
        json!([{ "id": "fast", "headers": { "h": "v" }, "body": { "k": 1 } }])
    );
    assert_eq!(model["request"]["headers"], json!({ "x": "1" }));
}
