//! Purpose: SQL parameter/column values and typed row access shared by every SQL consumer.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- storage-api/src/error.rs
//! External:
//! |-- serde::{Serialize, de::DeserializeOwned}
//! `-- serde_json
//! Used by:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! `-- storage-api/src/sql.rs
//! Invariants: JSON columns (Drizzle `text({ mode: "json" })`) are stored as `JSON.stringify` text; booleans as 0/1 integers.
//! Contract tests: storage-api/src/value.rs

use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Result, StorageError};

/// One SQLite storage-class value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    /// Encode a value as a Drizzle JSON-mode text column (compact `JSON.stringify` output).
    pub fn json<T: Serialize + ?Sized>(value: &T) -> Result<Value> {
        serde_json::to_string(value)
            .map(Value::Text)
            .map_err(|error| StorageError::Validation(format!("json encode: {error}")))
    }

    /// Encode an optional JSON column, `None` becomes SQL NULL.
    pub fn json_opt<T: Serialize>(value: Option<&T>) -> Result<Value> {
        match value {
            Some(value) => Value::json(value),
            None => Ok(Value::Null),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(value: $t) -> Self { Value::Integer(i64::from(value)) }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Integer(i64::from(value))
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Real(value)
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_owned())
    }
}
impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Value::Text(value.clone())
    }
}
impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Value::Blob(value)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Null, Into::into)
    }
}

/// Decode a column value into a Rust type.
pub trait FromValue: Sized {
    fn from_value(column: &str, value: &Value) -> Result<Self>;
}

fn mismatch(column: &str, expected: &str, value: &Value) -> StorageError {
    StorageError::Decode {
        column: column.to_owned(),
        message: format!("expected {expected}, got {value:?}"),
    }
}

impl FromValue for Value {
    fn from_value(_: &str, value: &Value) -> Result<Self> {
        Ok(value.clone())
    }
}
impl FromValue for i64 {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Integer(v) => Ok(*v),
            _ => Err(mismatch(column, "integer", value)),
        }
    }
}
impl FromValue for f64 {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Real(v) => Ok(*v),
            #[allow(clippy::cast_precision_loss)]
            Value::Integer(v) => Ok(*v as f64),
            _ => Err(mismatch(column, "real", value)),
        }
    }
}
impl FromValue for bool {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Integer(v) => Ok(*v != 0),
            _ => Err(mismatch(column, "boolean integer", value)),
        }
    }
}
impl FromValue for String {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Text(v) => Ok(v.clone()),
            _ => Err(mismatch(column, "text", value)),
        }
    }
}
impl FromValue for Vec<u8> {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Blob(v) => Ok(v.clone()),
            Value::Text(v) => Ok(v.as_bytes().to_vec()),
            _ => Err(mismatch(column, "blob", value)),
        }
    }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(column: &str, value: &Value) -> Result<Self> {
        match value {
            Value::Null => Ok(None),
            other => T::from_value(column, other).map(Some),
        }
    }
}

/// One result row. Column names are shared across rows of the same statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    columns: Arc<[String]>,
    values: Vec<Value>,
}

impl Row {
    pub fn new(columns: Arc<[String]>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn into_values(self) -> Vec<Value> {
        self.values
    }

    /// Raw value by column name.
    pub fn value(&self, column: &str) -> Result<&Value> {
        self.columns
            .iter()
            .position(|name| name == column)
            .map(|index| &self.values[index])
            .ok_or_else(|| StorageError::Decode {
                column: column.to_owned(),
                message: "no such column in result".into(),
            })
    }

    /// Typed value by column name.
    pub fn get<T: FromValue>(&self, column: &str) -> Result<T> {
        T::from_value(column, self.value(column)?)
    }

    /// Typed value by position.
    pub fn get_index<T: FromValue>(&self, index: usize) -> Result<T> {
        let value = self.values.get(index).ok_or_else(|| StorageError::Decode {
            column: index.to_string(),
            message: "column index out of range".into(),
        })?;
        let name = self.columns.get(index).map_or("?", String::as_str);
        T::from_value(name, value)
    }

    /// Decode a Drizzle JSON-mode text column.
    pub fn get_json<T: DeserializeOwned>(&self, column: &str) -> Result<T> {
        let text: String = self.get(column)?;
        serde_json::from_str(&text).map_err(|error| StorageError::Decode {
            column: column.to_owned(),
            message: error.to_string(),
        })
    }

    /// Decode a nullable Drizzle JSON-mode text column.
    pub fn get_json_opt<T: DeserializeOwned>(&self, column: &str) -> Result<Option<T>> {
        match self.get::<Option<String>>(column)? {
            None => Ok(None),
            Some(text) => {
                serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|error| StorageError::Decode {
                        column: column.to_owned(),
                        message: error.to_string(),
                    })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_access() {
        let row = Row::new(
            vec![
                "id".to_owned(),
                "n".to_owned(),
                "data".to_owned(),
                "gone".to_owned(),
            ]
            .into(),
            vec![
                Value::from("a"),
                Value::from(3_i64),
                Value::json(&serde_json::json!({"b": 1, "a": 2})).unwrap(),
                Value::Null,
            ],
        );
        assert_eq!(row.get::<String>("id").unwrap(), "a");
        assert_eq!(row.get::<i64>("n").unwrap(), 3);
        assert!(row.get::<bool>("n").unwrap());
        assert_eq!(row.get::<Option<String>>("gone").unwrap(), None);
        let data: serde_json::Value = row.get_json("data").unwrap();
        assert_eq!(data.to_string(), r#"{"b":1,"a":2}"#);
        assert!(matches!(
            row.get::<i64>("id"),
            Err(StorageError::Decode { .. })
        ));
        assert!(row.get::<i64>("missing").is_err());
    }
}
