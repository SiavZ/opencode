//! Purpose: project identity, workspace lifecycle and the Ref-keyed location registry
//! (packages/core/src/location*.ts, project.ts, workspace.ts and the legacy persistence in
//! packages/opencode/src/project/project.ts and control-plane/workspace.ts).
//! Scope: crate root and re-exports.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/git.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/project.rs
//! |-- locations/src/project_store.rs
//! |-- locations/src/registry.rs
//! |-- locations/src/scope.rs
//! `-- locations/src/workspace/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- locations/tests/project.rs
//! |-- locations/tests/registry.rs
//! |-- locations/tests/registry_log.rs
//! |-- locations/tests/workspace_lifecycle.rs
//! `-- locations/tests/workspace_sync.rs
//! Invariants: this crate never depends on session or tool crates; the per-location bundle is a
//! generic `S` injected by `crates/runtime`.
//! Contract tests: locations/tests/registry.rs, locations/tests/project.rs, locations/tests/workspace_lifecycle.rs, locations/tests/workspace_sync.rs

pub mod error;
pub mod git;
pub mod ports;
pub mod project;
pub mod project_store;
pub mod registry;
pub mod scope;
pub mod workspace;

pub use error::{LocationError, Result};
pub use opencode_schema::location::{Info as LocationInfo, Ref as LocationRef};
pub use project::{GitProjectResolver, ProjectResolver, Resolved, Vcs};
pub use project_store::{FromDirectory, ProjectStore};
pub use registry::{
    BuildContext, DEFAULT_IDLE_TTL, LocationFactory, LocationLease, LocationRegistry,
    LocationScope, LocationServices, RegistryOptions,
};
pub use workspace::WorkspaceService;
