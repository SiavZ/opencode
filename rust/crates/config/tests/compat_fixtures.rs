//! Purpose: V2-compat fixtures copied verbatim from `packages/opencode/test/config/fixtures/v2-compat`:
//! read lowering (`ConfigV2Compat.lower` + V1 decode), `updateGlobal` exact written text and
//! normalized info, and project `update` written text. All filesystem work in temp dirs.
//! Scope: integration tests, fixture-based.
//! Uses:
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/parse.rs
//! `-- config/src/legacy/update.rs
//! External:
//! |-- serde_json::Value
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: comparisons follow the TS `snapshot` helper (JSONC-parsed equality), plus exact
//! text for JSONC update outputs where comments must survive.
//! Contract tests: config/tests/compat_fixtures.rs

use opencode_config::legacy::{self, parse};
use opencode_config::{Environment, schema};
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v2-compat")
}

fn jsonc(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).expect("read fixture");
    parse::parse_jsonc(&text, &path.display().to_string()).expect("fixture parses")
}

/// `ConfigParse.schema(ConfigV1.Info, patch)` like the TS fixture runner.
fn v1(value: &Value) -> Value {
    schema::decode(schema::v1::info(), value).expect("patch decodes")
}

fn cases(dir: &Path, suffix: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("dir")
        .filter_map(|e| {
            e.ok()?
                .file_name()
                .to_str()?
                .strip_suffix(suffix)
                .map(str::to_string)
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixtures in {}", dir.display());
    names
}

#[test]
fn read_fixtures_lower_and_decode_like_typescript() {
    let dir = fixtures().join("read");
    for name in cases(&dir, "-input.jsonc") {
        let input = jsonc(&dir.join(format!("{name}-input.jsonc")));
        let original = input.clone();
        let mut diags = parse::Diagnostics::default();
        let decoded = parse::decode_config(&input, "test:v2-compat", &mut diags).expect(&name);
        assert_eq!(input, original, "{name}: lowering must not mutate input");
        assert_eq!(
            decoded,
            jsonc(&dir.join(format!("{name}-output.json"))),
            "{name}"
        );
    }
}

#[test]
fn global_update_fixtures_match_written_text_and_info() {
    let dir = fixtures().join("update-global");
    let mut inputs: Vec<(String, String)> = Vec::new();
    for ext in ["json", "jsonc"] {
        inputs.extend(
            cases(&dir, &format!("-input.{ext}"))
                .into_iter()
                .map(|n| (n, ext.to_string())),
        );
    }
    for (name, ext) in inputs {
        let home = tempfile::tempdir().expect("tmp");
        let env = Environment::isolated(home.path());
        std::fs::create_dir_all(&env.global_config).expect("mkdir");
        let file = env.global_config.join(format!("opencode.{ext}"));
        std::fs::copy(dir.join(format!("{name}-input.{ext}")), &file).expect("copy");
        let patch = v1(&jsonc(&dir.join(format!("{name}-patch.json"))));
        let updated = legacy::update_global(&env, &patch).expect(&name);
        let written = std::fs::read_to_string(&file).expect("written");
        let expected_text =
            std::fs::read_to_string(dir.join(format!("{name}-output.{ext}"))).expect("out");
        if ext == "jsonc" {
            assert_eq!(written, expected_text, "{name}: exact jsonc text");
        }
        assert_eq!(
            parse::parse_jsonc(&written, "w").expect("w"),
            parse::parse_jsonc(&expected_text, "e").expect("e")
        );
        assert_eq!(
            updated.info,
            jsonc(&dir.join(format!("{name}-normalized.json"))),
            "{name}: info"
        );
        assert!(updated.changed, "{name}: changed");
    }
}

#[test]
fn project_update_fixtures_match_written_text_and_normalized() {
    let dir = fixtures().join("update-project");
    for name in cases(&dir, "-input.json") {
        let home = tempfile::tempdir().expect("tmp");
        let project = tempfile::tempdir().expect("tmp");
        let env = Environment::isolated(home.path());
        let file = project.path().join("config.json");
        std::fs::copy(dir.join(format!("{name}-input.json")), &file).expect("copy");
        let patch = v1(&jsonc(&dir.join(format!("{name}-patch.json"))));
        legacy::update(project.path(), &patch, &env).expect(&name);
        let written = jsonc(&file);
        assert_eq!(
            written,
            jsonc(&dir.join(format!("{name}-output.json"))),
            "{name}: written"
        );
        let lowered = legacy::lower(&written, "file").expect("lower");
        let normalized =
            parse::decode_config(&lowered.value, "file", &mut Default::default()).expect("decode");
        assert_eq!(
            normalized,
            jsonc(&dir.join(format!("{name}-normalized.json"))),
            "{name}: normalized"
        );
    }
}
