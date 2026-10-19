//! Purpose: shared helpers for the migration conformance tests (bun-like connection, row dumps).
//! Scope: test support module, included with `mod common;`.
//! Uses:
//! rust/crates/
//! `-- storage-sqlite/src/migrations/mod.rs
//! External:
//! |-- rusqlite::Connection
//! |-- rusqlite::types::ValueRef::Blob
//! |-- rusqlite::types::ValueRef::Integer
//! |-- rusqlite::types::ValueRef::Null
//! |-- rusqlite::types::ValueRef::Real
//! |-- rusqlite::types::ValueRef::Text
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::Value::Array
//! |-- serde_json::Value::Null
//! |-- serde_json::Value::Object
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/tests/migration.rs
//! `-- storage-sqlite/tests/migration_data.rs
//! Invariants: helpers only read state, they never apply migrations themselves.
//! Contract tests: storage-sqlite/tests/migration.rs, storage-sqlite/tests/migration_data.rs
#![allow(dead_code)]

use opencode_storage_sqlite::migrations::{self, Migration};
use rusqlite::Connection;

/// Plain connection with bun:sqlite defaults. The bundled rusqlite engine is compiled with
/// SQLITE_DEFAULT_FOREIGN_KEYS=1, bun:sqlite is not, and the TS tests rely on the latter.
pub fn mem() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

pub fn one(id: &str) -> Vec<Migration> {
    vec![migrations::get(id).unwrap().clone()]
}

pub fn strings(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

pub fn json(conn: &Connection, sql: &str) -> serde_json::Value {
    let mut stmt = conn.prepare(sql).unwrap();
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let rows = stmt
        .query_map([], |row| {
            let mut map = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                    rusqlite::types::ValueRef::Integer(v) => v.into(),
                    rusqlite::types::ValueRef::Real(v) => v.into(),
                    rusqlite::types::ValueRef::Text(v) => {
                        String::from_utf8_lossy(v).into_owned().into()
                    }
                    rusqlite::types::ValueRef::Blob(_) => "<blob>".into(),
                };
                map.insert(name.clone(), value);
            }
            Ok(serde_json::Value::Object(map))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    serde_json::Value::Array(rows)
}
