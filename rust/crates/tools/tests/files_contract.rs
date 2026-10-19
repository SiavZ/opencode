//! Purpose: conformance of bundled file descriptions and captured schema fixtures.
//! Scope: fixture-based and baseline-source contract checks.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! |-- schemas::CORE_SCHEMAS
//! |-- schemas::LEGACY_SCHEMAS
//! |-- serde_json::Value
//! `-- serde_json::from_str
//! Used by:
//! `-- none, entrypoint
//! Invariants: no fixture can silently drift from the baseline tool description bytes.
//! Contract tests: this integration suite

use opencode_tools::files::{Generation, schemas};
use serde_json::Value;
use std::path::Path;

fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

#[test]
fn legacy_descriptions_are_byte_identical_to_baseline() {
    for name in ["read", "write", "edit", "apply_patch"] {
        let source =
            std::fs::read(repository().join(format!("packages/opencode/src/tool/{name}.txt")))
                .unwrap();
        assert_eq!(
            schemas::description(Generation::Legacy, name).as_bytes(),
            source,
            "{name}"
        );
    }
}

#[test]
fn core_descriptions_match_baseline_literals() {
    for name in ["read", "write", "edit", "apply_patch"] {
        let filename = if name == "apply_patch" {
            "apply-patch"
        } else {
            name
        };
        let source = std::fs::read_to_string(
            repository().join(format!("packages/core/src/tool/{filename}.ts")),
        )
        .unwrap();
        let literal = format!("\"{}\"", schemas::description(Generation::Core, name));
        assert!(
            source.contains(&literal),
            "{name} description differs from baseline"
        );
    }
}

#[test]
fn schemas_are_the_captured_baseline_fixtures() {
    let core: Value = serde_json::from_str(schemas::CORE_SCHEMAS).unwrap();
    let legacy: Value = serde_json::from_str(schemas::LEGACY_SCHEMAS).unwrap();
    for name in ["read", "write", "edit", "apply_patch"] {
        assert_eq!(
            schemas::input_schema(Generation::Core, name),
            core[name]["input"]
        );
        assert_eq!(
            schemas::output_schema(Generation::Core, name),
            Some(core[name]["output"].clone())
        );
        assert_eq!(
            schemas::input_schema(Generation::Legacy, name),
            legacy[name]
        );
        assert_eq!(schemas::output_schema(Generation::Legacy, name), None);
        let path = if name == "apply_patch" {
            "patchText"
        } else {
            "path"
        };
        assert_eq!(core[name]["input"]["properties"][path]["type"], "string");
        let path = if name == "apply_patch" {
            "patchText"
        } else {
            "filePath"
        };
        assert_eq!(legacy[name]["properties"][path]["type"], "string");
    }
    assert_eq!(
        core["edit"]["input"]["required"],
        serde_json::json!(["path", "oldString", "newString"])
    );
    assert_eq!(legacy["read"]["properties"]["offset"]["minimum"], 0);
    assert_eq!(
        core["read"]["input"]["properties"]["offset"]["anyOf"][0]["type"],
        "integer"
    );
    assert_eq!(
        core["read"]["input"]["properties"]["limit"]["anyOf"][0]["allOf"][1]["maximum"],
        2000
    );
    assert_eq!(core["edit"]["input"]["additionalProperties"], false);
    assert_eq!(
        core["edit"]["output"]["$defs"]["SnapshotFileDiff"]["properties"]["additions"]["type"],
        "number"
    );
    assert_eq!(legacy["read"]["properties"]["offset"]["type"], "integer");
}
