//! Purpose: baseline file-tool descriptions and captured JSON schema wire contracts.
//! Scope: immutable bundled fixtures, no runtime I/O.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/common.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- tools/src/files/mod.rs
//! Invariants: legacy descriptions are byte-identical to baseline text files.
//! Contract tests: tools/tests/files_contract.rs

use super::common::Generation;
use serde_json::Value;
use std::sync::OnceLock;

pub const READ_DESCRIPTION: &str = include_str!("descriptions/read.txt");
pub const WRITE_DESCRIPTION: &str = include_str!("descriptions/write.txt");
pub const EDIT_DESCRIPTION: &str = include_str!("descriptions/edit.txt");
pub const APPLY_DESCRIPTION: &str = include_str!("descriptions/apply_patch.txt");
pub const CORE_SCHEMAS: &str = include_str!("fixtures/core-schemas.json");
pub const LEGACY_SCHEMAS: &str = include_str!("fixtures/legacy-schemas.json");

fn schemas(generation: Generation) -> &'static Value {
    static CORE: OnceLock<Value> = OnceLock::new();
    static LEGACY: OnceLock<Value> = OnceLock::new();
    let (storage, source) = match generation {
        Generation::Core => (&CORE, CORE_SCHEMAS),
        Generation::Legacy => (&LEGACY, LEGACY_SCHEMAS),
    };
    storage.get_or_init(|| serde_json::from_str(source).expect("validated bundled schema fixture"))
}

pub fn input_schema(generation: Generation, name: &str) -> Value {
    match generation {
        Generation::Core => schemas(generation)[name]["input"].clone(),
        Generation::Legacy => schemas(generation)[name].clone(),
    }
}

pub fn output_schema(generation: Generation, name: &str) -> Option<Value> {
    let schema = &schemas(generation)[name]["output"];
    if generation == Generation::Legacy || schema.is_null() {
        return None;
    }
    Some(schema.clone())
}

pub fn description(generation: Generation, name: &str) -> &'static str {
    match (generation, name) {
        (Generation::Legacy, "read") => READ_DESCRIPTION,
        (Generation::Legacy, "write") => WRITE_DESCRIPTION,
        (Generation::Legacy, "edit") => EDIT_DESCRIPTION,
        (Generation::Legacy, "apply_patch") => APPLY_DESCRIPTION,
        (Generation::Core, "read") => {
            "Read a text file or supported image, page through a large UTF-8 text file by line offset, or list a directory page. Relative paths resolve from the current location; absolute paths inside it are accepted, while external absolute paths require external_directory approval."
        }
        (Generation::Core, "write") => {
            "Write content to one file. Relative paths resolve within the active Location. Absolute paths inside the Location are accepted. Explicit external absolute paths require external_directory approval before edit approval."
        }
        (Generation::Core, "edit") => {
            "Replace exact text in one file. Relative paths resolve within the active Location. Absolute paths inside the Location are accepted. Explicit external absolute paths require external_directory approval before edit approval."
        }
        (Generation::Core, "apply_patch") => {
            "Apply one patch containing add, update, and delete file operations. All targets are resolved and approved before target contents are read. Operations apply sequentially; if a later operation fails, earlier operations remain applied and the failure reports them explicitly. Moves and atomic rollback are not supported yet."
        }
        _ => "",
    }
}
