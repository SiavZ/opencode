//! Purpose: crate root for Rust-owned provider adapters: protocol families, transports,
//! route composition, auth, provider registrations, credentials and catalog/model resolution.
//! Scope: native. Consumers use `opencode_llm_api` traits, this crate supplies implementations.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/cache_policy.rs
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/src/credentials/mod.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/mod.rs
//! |-- provider-adapters/src/providers/mod.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/shared.rs
//! |-- provider-adapters/src/tool_schema.rs
//! |-- provider-adapters/src/transport/mod.rs
//! `-- provider-adapters/src/unsupported.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: no dependency on session/server/UI crates. Live provider calls only in
//! explicitly labelled live tests (none today).
//! Contract tests: provider-adapters/src/route.rs, provider-adapters/tests/
//! Module ownership: lib.rs, auth.rs, cache_policy.rs, protocol.rs, route.rs, shared.rs, tool_schema.rs, unsupported.rs are the
//! p03 parent's. transport/ -> p03.transport, protocols/ -> p03.openai + p03.messages,
//! providers/ -> p03.providers, credentials/ -> p03.auth, catalog/ -> p03.catalog.

pub mod auth;
pub mod cache_policy;
pub mod catalog;
pub mod credentials;
pub mod protocol;
pub mod protocols;
pub mod providers;
pub mod route;
pub mod shared;
pub mod tool_schema;
pub mod transport;
pub mod unsupported;
