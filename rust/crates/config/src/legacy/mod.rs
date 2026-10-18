//! Purpose: legacy (V1) merged configuration service surface: load, get, directories, console
//! state, `update`, `updateGlobal`, plus compat lowering and markdown loaders.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- config/src/legacy/compat.rs
//! |-- config/src/legacy/compat_mcp.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/legacy/markdown.rs
//! |-- config/src/legacy/parse.rs
//! |-- config/src/legacy/plugin.rs
//! |-- config/src/legacy/remote.rs
//! |-- config/src/legacy/steps.rs
//! `-- config/src/legacy/update.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- config/src/lib.rs
//! Invariants: see `load.rs` and `update.rs`.
//! Contract tests: config/tests/legacy_load.rs

pub mod compat;
mod compat_mcp;
pub mod load;
pub mod markdown;
pub mod parse;
pub mod plugin;
pub mod remote;
mod steps;
pub mod update;

pub use compat::{Diagnostic, DiagnosticKind, Lowered, lower};
pub use load::{ConsoleState, Instance, LegacyState, Sources, load};
pub use parse::{Diagnostics, SCHEMA_URL, SourcedDiagnostic};
pub use plugin::{Origin, Scope};
pub use remote::{
    AccountOrg, AccountSource, ManagedPreferences, NoAccounts, RemoteFetcher, RemoteResponse,
    SystemManagedPreferences, UnsupportedRemote, WellKnownAuth,
};
pub use steps::{directories, project_files};
pub use update::{GlobalUpdate, update, update_global};
