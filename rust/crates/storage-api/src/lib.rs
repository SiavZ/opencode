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
//! rust/crates/
//! |-- context/src/epoch.rs
//! |-- context/src/history.rs
//! |-- context/src/source/error.rs
//! |-- context/tests/epoch.rs
//! |-- context/tests/history.rs
//! |-- context/tests/support/events.rs
//! |-- context/tests/support/mod.rs
//! |-- event-api/src/store.rs
//! |-- event-store/src/commit.rs
//! |-- event-store/src/read.rs
//! |-- event-store/src/store.rs
//! |-- event-store/tests/commit.rs
//! |-- event-store/tests/owners.rs
//! |-- event-store/tests/replay.rs
//! |-- event-store/tests/rows.rs
//! |-- event-store/tests/streams.rs
//! |-- event-store/tests/support/mod.rs
//! |-- locations/src/error.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/project_store.rs
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/sync.rs
//! |-- locations/tests/project.rs
//! |-- locations/tests/support/mod.rs
//! |-- locations/tests/support/workspace.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/legacy.rs
//! |-- runtime/src/location_models.rs
//! |-- runtime/src/process.rs
//! |-- runtime/tests/production.rs
//! |-- runtime/tests/production_compaction.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/children/store.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/projection/adapter.rs
//! |-- session-runtime/src/projection/legacy.rs
//! |-- session-runtime/src/projection/updater.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/runner/turn/request.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/src/todo.rs
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! |-- session-runtime/tests/admission_projection.rs
//! |-- session-runtime/tests/children/fixture.rs
//! |-- session-runtime/tests/support/legacy.rs
//! |-- session-runtime/tests/support/publish.rs
//! |-- session-runtime/tests/support/sql.rs
//! |-- session-runtime/tests/todo.rs
//! |-- storage-sqlite/src/connection.rs
//! |-- storage-sqlite/src/legacy_files/auth.rs
//! |-- storage-sqlite/src/legacy_files/store.rs
//! |-- storage-sqlite/src/migration.rs
//! |-- storage-sqlite/src/migrations/mod.rs
//! |-- storage-sqlite/src/repositories/credentials.rs
//! |-- storage-sqlite/src/repositories/domains.rs
//! |-- storage-sqlite/src/repositories/projects.rs
//! |-- storage-sqlite/src/transaction.rs
//! |-- storage-sqlite/tests/credentials.rs
//! |-- storage-sqlite/tests/legacy_json.rs
//! |-- storage-sqlite/tests/repositories.rs
//! |-- storage-sqlite/tests/repositories_accounts.rs
//! |-- storage-sqlite/tests/repositories_interop.rs
//! |-- storage-sqlite/tests/support/mod.rs
//! |-- storage-sqlite/tests/transactions.rs
//! `-- storage-sqlite/tests/ts_interop.rs
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
