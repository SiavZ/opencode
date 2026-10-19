//! Purpose: SQL-backed repositories for the baseline tables owned outside the session runtime:
//! project + project_directory, permission, workspace, account + account_state + control_account,
//! session_share and data_migration. Credentials live in `credentials.rs`.
//! Scope: stateless functions over `&dyn Sql`, so callers compose them inside one unit of work
//! (`Database::transaction`), exactly like the TS services pass `tx ?? db`.
//! Uses:
//! rust/crates/
//! |-- storage-sqlite/src/repositories/credentials.rs
//! |-- storage-sqlite/src/repositories/domains.rs
//! `-- storage-sqlite/src/repositories/projects.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/lib.rs
//! |-- storage-sqlite/tests/repositories.rs
//! |-- storage-sqlite/tests/repositories_accounts.rs
//! `-- storage-sqlite/tests/repositories_interop.rs
//! Invariants: each function issues the same statements, filters, ordering and conflict behavior as the
//! TS call site named in its doc comment. Session-table semantics are Phase 04's.
//! Contract tests: storage-sqlite/tests/repositories.rs, storage-sqlite/tests/credentials.rs

pub mod credentials;
mod domains;
mod projects;

pub use domains::{account, data_migration, permission, session_share, workspace};
pub use projects::{project, project_directory};
