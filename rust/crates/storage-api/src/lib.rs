//! Purpose: crate root for portable storage contracts: SQL executor, transactions and savepoints,
//! typed errors, row values, legacy JSON store and baseline table records.
//! Scope: pure values and async service traits. No engine, file or process code.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! |-- storage-api/src/json_store.rs
//! |-- storage-api/src/records/mod.rs
//! |-- storage-api/src/sql.rs
//! `-- storage-api/src/value.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-02, consumers 02/04/05/06 added as crates land)
//! Invariants: portable (native + wasm), depends on no first-party crate except schema.
//! Contract tests: storage-api/src/sql.rs, storage-api/src/value.rs, storage-api/src/error.rs

pub mod error;
pub mod json_store;
pub mod records;
pub mod sql;
pub mod value;

pub use error::{BoxError, Result, StorageError};
pub use json_store::JsonStore;
pub use sql::{Access, Behavior, Database, DatabaseExt, Sql, SqlExt, TxOptions, Work, WorkOutcome};
pub use value::{FromValue, Row, Value};
