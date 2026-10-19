//! Purpose: crate root for the native SQLite storage adapter on the baseline Drizzle schema and the
//! surviving legacy JSON store.
//! Scope: process-global connection worker, transactions, migrations, repositories, legacy files.
//! Uses:
//! rust/crates/
//! |-- storage-sqlite/src/connection.rs
//! |-- storage-sqlite/src/legacy_files.rs
//! |-- storage-sqlite/src/migration.rs
//! |-- storage-sqlite/src/migrations/mod.rs
//! |-- storage-sqlite/src/repositories/mod.rs
//! `-- storage-sqlite/src/transaction.rs
//! External:
//! `-- rusqlite::version
//! Used by:
//! rust/crates/
//! |-- event-store/tests/rows.rs
//! |-- event-store/tests/support/mod.rs
//! |-- locations/tests/support/mod.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/process.rs
//! |-- session-runtime/tests/support/sql.rs
//! |-- session-runtime/tests/todo.rs
//! |-- storage-sqlite/tests/credentials.rs
//! |-- storage-sqlite/tests/repositories_interop.rs
//! |-- storage-sqlite/tests/support/mod.rs
//! |-- storage-sqlite/tests/transactions.rs
//! `-- storage-sqlite/tests/ts_interop.rs
//! Invariants: same schema and file as the TypeScript baseline, no schema fork (decision D5).
//! Contract tests: storage-sqlite/tests/
//! Ownership: p02.repos owns legacy_files.rs and repositories/**, p02.sqlite every other module.

pub mod connection;
pub mod legacy_files;
pub mod migration;
pub mod migrations;
pub mod repositories;
mod transaction;

pub use connection::{BASELINE_PRAGMAS, OpenOptions, PathEnv, SqliteDatabase, database_path};
pub use transaction::map_error;

/// Version of the bundled SQLite engine, e.g. "3.53.2".
pub fn sqlite_version() -> &'static str {
    rusqlite::version()
}
