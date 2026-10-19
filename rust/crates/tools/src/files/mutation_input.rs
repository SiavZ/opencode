//! Purpose: input decoding for the edit and write tools of both generations, with the baseline
//! Effect Schema first-error messages, plus the legacy invalid-arguments wrapper prose.
//! Scope: `EditTool.Input`/`WriteTool.Input` (core) and `Parameters` (legacy) decoding.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/lib.rs
//! `-- tools/src/files/read_common.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/mod.rs
//! `-- tools/src/files/write.rs
//! Invariants: keys are checked in schema declaration order and the first failure wins, optional
//! booleans reject `null` (`Expected boolean | undefined, got null`), excess keys are ignored.
//! Contract tests: tools/tests/files_edit.rs, tools/tests/files_write.rs

use super::read_common::format_unknown;
use opencode_tool_api::ToolFailure;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    String(&'static str),
    OptionalBool(&'static str),
}

/// Decoded field values in declaration order. Missing optional booleans are `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    String(String),
    Bool(Option<bool>),
}

pub const CORE_EDIT: &[Field] = &[
    Field::String("path"),
    Field::String("oldString"),
    Field::String("newString"),
    Field::OptionalBool("replaceAll"),
];
pub const LEGACY_EDIT: &[Field] = &[
    Field::String("filePath"),
    Field::String("oldString"),
    Field::String("newString"),
    Field::OptionalBool("replaceAll"),
];
pub const CORE_WRITE: &[Field] = &[Field::String("path"), Field::String("content")];
pub const LEGACY_WRITE: &[Field] = &[Field::String("content"), Field::String("filePath")];

/// Decode against `fields`, returning the Effect Schema issue text on the first failure.
pub fn decode(input: &Value, fields: &[Field]) -> Result<Vec<Decoded>, String> {
    let Value::Object(object) = input else {
        return Err(format!("Expected object, got {}", format_unknown(input)));
    };
    fields
        .iter()
        .map(|field| match *field {
            Field::String(key) => match object.get(key) {
                None => Err(format!("Missing key\n  at [\"{key}\"]")),
                Some(Value::String(value)) => Ok(Decoded::String(value.clone())),
                Some(other) => Err(format!(
                    "Expected string, got {}\n  at [\"{key}\"]",
                    format_unknown(other)
                )),
            },
            Field::OptionalBool(key) => match object.get(key) {
                None => Ok(Decoded::Bool(None)),
                Some(Value::Bool(value)) => Ok(Decoded::Bool(Some(*value))),
                Some(other) => Err(format!(
                    "Expected boolean | undefined, got {}\n  at [\"{key}\"]",
                    format_unknown(other)
                )),
            },
        })
        .collect()
}

/// Core `Tool.make` decode failure.
pub fn core_failure(detail: String) -> ToolFailure {
    ToolFailure::invalid_input(detail)
}

/// Legacy `Tool.define` `InvalidArgumentsError` prose (`String(error)` of a SchemaError).
pub fn legacy_failure(tool: &str, detail: String) -> ToolFailure {
    ToolFailure::new(format!(
        "The {tool} tool was called with invalid arguments: SchemaError({detail}).\nPlease rewrite the input so it satisfies the expected schema."
    ))
}

/// Edit input `{ path | filePath, oldString, newString, replaceAll? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

pub fn edit_input(values: Vec<Decoded>) -> EditInput {
    let mut values = values.into_iter();
    let mut text = || match values.next() {
        Some(Decoded::String(value)) => value,
        _ => String::new(),
    };
    let (path, old_string, new_string) = (text(), text(), text());
    let replace_all = match values.next() {
        Some(Decoded::Bool(value)) => value,
        _ => None,
    };
    EditInput {
        path,
        old_string,
        new_string,
        replace_all,
    }
}

/// `(path, content)` for either write generation (legacy declares content first).
pub fn write_input(values: Vec<Decoded>, legacy: bool) -> (String, String) {
    let mut strings = values.into_iter().map(|value| match value {
        Decoded::String(value) => value,
        Decoded::Bool(_) => String::new(),
    });
    let (first, second) = (
        strings.next().unwrap_or_default(),
        strings.next().unwrap_or_default(),
    );
    if legacy {
        (second, first)
    } else {
        (first, second)
    }
}
