//! Purpose: Drizzle column codecs (text, integer, real, boolean mode, JSON mode, database/path.ts
//! custom path columns) shared by every baseline row record.
//! Scope: pure value conversion between Rust fields and SQLite [`Value`]s.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! |-- storage-api/src/records/path.rs
//! |-- storage-api/src/records/secret.rs
//! `-- storage-api/src/value.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! rust/crates/
//! `-- storage-api/src/records/mod.rs
//! Invariants: JSON mode is compact `JSON.stringify`, boolean mode writes 0/1 and reads `Number(v) === 1`,
//! path columns follow packages/core/src/database/path.ts (posix: absolute means a leading `/`; windows:
//! backslashes are stored as `/` and drive or UNC paths are converted back on read).
//! Contract tests: storage-sqlite/tests/repositories.rs

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Result, StorageError};
use crate::value::{Row, Value};

fn decode_error(column: &str, message: impl Into<String>) -> StorageError {
    StorageError::Decode {
        column: column.to_owned(),
        message: message.into(),
    }
}

pub mod text {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<String> {
        row.get(column)
    }
    pub fn encode(value: &str) -> Result<Value> {
        Ok(Value::Text(value.to_owned()))
    }
}

pub mod opt_text {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<Option<String>> {
        row.get(column)
    }
    pub fn encode(value: &Option<String>) -> Result<Value> {
        Ok(value.clone().map_or(Value::Null, Value::Text))
    }
}

pub mod int {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<i64> {
        row.get(column)
    }
    pub fn encode(value: &i64) -> Result<Value> {
        Ok(Value::Integer(*value))
    }
}

pub mod opt_int {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<Option<i64>> {
        row.get(column)
    }
    pub fn encode(value: &Option<i64>) -> Result<Value> {
        Ok(value.map_or(Value::Null, Value::Integer))
    }
}

pub mod real {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<f64> {
        row.get(column)
    }
    pub fn encode(value: &f64) -> Result<Value> {
        Ok(Value::Real(*value))
    }
}

/// `integer({ mode: "boolean" })`: drizzle reads `Number(value) === 1`.
pub mod opt_boolean {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<Option<bool>> {
        Ok(match row.value(column)? {
            Value::Null => None,
            Value::Integer(v) => Some(*v == 1),
            Value::Real(v) => Some(*v == 1.0),
            Value::Text(v) => Some(v.trim().parse::<f64>().is_ok_and(|n| n == 1.0)),
            Value::Blob(_) => Some(false),
        })
    }
    pub fn encode(value: &Option<bool>) -> Result<Value> {
        Ok(value.map_or(Value::Null, |v| Value::Integer(i64::from(v))))
    }
}

pub mod boolean {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<bool> {
        super::opt_boolean::decode(row, column)?
            .ok_or_else(|| decode_error(column, "unexpected NULL"))
    }
    pub fn encode(value: &bool) -> Result<Value> {
        Ok(Value::Integer(i64::from(*value)))
    }
}

/// `text({ mode: "json" }).notNull()`.
pub mod json {
    use super::*;
    pub fn decode<T: DeserializeOwned>(row: &Row, column: &str) -> Result<T> {
        row.get_json(column)
    }
    pub fn encode<T: Serialize>(value: &T) -> Result<Value> {
        Value::json(value)
    }
}

/// Nullable `text({ mode: "json" })`.
pub mod opt_json {
    use super::*;
    pub fn decode<T: DeserializeOwned>(row: &Row, column: &str) -> Result<Option<T>> {
        row.get_json_opt(column)
    }
    pub fn encode<T: Serialize>(value: &Option<T>) -> Result<Value> {
        Value::json_opt(value.as_ref())
    }
}

/// `DatabasePath.absoluteColumn()`.
pub mod absolute {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<String> {
        let text: String = row.get(column)?;
        crate::records::path::from_driver(&text).map_err(|m| decode_error(column, m))
    }
    pub fn encode(value: &str) -> Result<Value> {
        crate::records::path::absolute(value)
            .map(Value::Text)
            .map_err(StorageError::Validation)
    }
}

/// `DatabasePath.directoryColumn()`: empty strings pass through untouched.
pub mod directory {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<String> {
        let text: String = row.get(column)?;
        if text.is_empty() {
            return Ok(text);
        }
        crate::records::path::from_driver(&text).map_err(|m| decode_error(column, m))
    }
    pub fn encode(value: &str) -> Result<Value> {
        if value.is_empty() {
            return Ok(Value::Text(String::new()));
        }
        super::absolute::encode(value)
    }
}

/// Nullable `DatabasePath.pathColumn()`.
pub mod opt_path {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<Option<String>> {
        Ok(row
            .get::<Option<String>>(column)?
            .map(|v| crate::records::path::storage_path(&v)))
    }
    pub fn encode(value: &Option<String>) -> Result<Value> {
        Ok(value.as_deref().map_or(Value::Null, |v| {
            Value::Text(crate::records::path::storage_path(v))
        }))
    }
}

/// `DatabasePath.absoluteArrayColumn()`.
pub mod absolute_array {
    use super::*;
    pub fn decode(row: &Row, column: &str) -> Result<Vec<String>> {
        let items: Vec<String> = row.get_json(column)?;
        items
            .iter()
            .map(|item| {
                crate::records::path::from_driver(item).map_err(|m| decode_error(column, m))
            })
            .collect()
    }
    pub fn encode(value: &[String]) -> Result<Value> {
        let items = value
            .iter()
            .map(|item| crate::records::path::absolute(item))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StorageError::Validation)?;
        Value::json(&items)
    }
}

/// Secret `text().notNull()`.
pub mod secret_text {
    use super::*;
    use crate::records::Secret;
    pub fn decode(row: &Row, column: &str) -> Result<Secret<String>> {
        row.get::<String>(column).map(Secret::new)
    }
    pub fn encode(value: &Secret<String>) -> Result<Value> {
        Ok(Value::Text(value.expose().clone()))
    }
}

/// Secret `text({ mode: "json" }).notNull()`.
pub mod secret_json {
    use super::*;
    use crate::records::Secret;
    pub fn decode<T: DeserializeOwned>(row: &Row, column: &str) -> Result<Secret<T>> {
        row.get_json(column).map(Secret::new)
    }
    pub fn encode<T: Serialize>(value: &Secret<T>) -> Result<Value> {
        Value::json(value.expose())
    }
}
