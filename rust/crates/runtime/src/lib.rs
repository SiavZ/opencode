//! Purpose: composition root for the Rust port (Phase 91). Builds the process-global services
//! (database, event store, Session service and execution, project and workspace services) once,
//! the Location-scoped bundle lazily per `Location.Ref` (60-minute idle TTL), and hands both to
//! the server, CLI and embedded hosts. Mirrors `packages/core/src/location-services.ts` and the
//! global nodes of `packages/core/src/session.ts`.
//! Scope: crate root and re-exports.
//! Uses:
//! rust/crates/
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/legacy.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/paths.rs
//! |-- runtime/src/process.rs
//! `-- server/src/lifecycle.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/commands/terminal.rs
//! |-- opencode/src/cli/commands/turn.rs
//! `-- opencode/src/serve.rs
//! rust/crates/
//! |-- runtime/tests/legacy.rs
//! |-- runtime/tests/process.rs
//! |-- runtime/tests/production.rs
//! `-- runtime/tests/production_compaction.rs
//! Invariants: this crate only wires constructors owned by domain crates; it holds no domain
//! logic. Every capability not yet composed is reported through the domain's typed
//! NotYetPorted/Unavailable error, never faked.
//! Contract tests: runtime/tests/process.rs

pub mod adapters;
pub mod execution_support;
pub mod legacy;
pub mod location;
pub mod paths;
pub mod process;

pub use location::{LocationBundle, LocationGlobals};
pub use opencode_server::lifecycle::{ListenOptions, Listener};
pub use paths::GlobalPaths;
pub use process::{Process, ProcessError, ProcessOptions};
