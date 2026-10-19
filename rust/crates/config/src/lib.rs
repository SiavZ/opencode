//! Purpose: configuration for the OpenCode port: the current ordered-document Config per
//! Location, the legacy merged Config with full discovery and precedence, JSONC parsing with
//! TypeScript-compatible errors, `{env:}`/`{file:}` substitution, provenance, and secret-safe
//! comment-preserving writes.
//! Scope: crate root.
//! Uses:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/env.rs
//! |-- config/src/error.rs
//! |-- config/src/fs.rs
//! |-- config/src/jsonc/mod.rs
//! |-- config/src/legacy/mod.rs
//! |-- config/src/migrate.rs
//! |-- config/src/provider.rs
//! |-- config/src/provider_options.rs
//! |-- config/src/resources.rs
//! |-- config/src/schema/mod.rs
//! |-- config/src/value.rs
//! `-- config/src/variable.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- config/tests/compat_fixtures.rs
//! |-- config/tests/current.rs
//! |-- config/tests/legacy_load.rs
//! |-- config/tests/legacy_remote.rs
//! |-- config/tests/legacy_update.rs
//! |-- config/tests/markdown.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_agents.rs
//! |-- runtime/src/location_models.rs
//! `-- runtime/src/location_tools.rs
//! Invariants: no function in this crate reads the real process environment except
//! `Environment::from_process`; tests always pass an isolated `Environment`.
//! Contract tests: config/tests/current.rs

pub mod current;
pub mod env;
pub mod error;
pub mod fs;
pub mod jsonc;
pub mod legacy;
pub mod migrate;
pub mod provider;
pub mod provider_options;
pub mod resources;
pub mod schema;
pub mod value;
pub mod variable;

pub use current::{CurrentConfig, Document, Entry, LocationSnapshot};
pub use env::{Environment, Platform};
pub use error::{ConfigError, Issue, RemoteError};
pub use value::Provenance;
