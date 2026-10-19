//! Purpose: crate root for the tool contract: definitions, invocation context, typed outcomes,
//! authorization/progress/output-bounding ports and the scoped registry with stale-call rejection.
//! Scope: portable contract plus the pure registry. Leaf tools live in `tools`.
//! Uses:
//! rust/crates/
//! |-- tool-api/src/context.rs
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/output.rs
//! `-- tool-api/src/registry.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_tools.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/tests/support/provider.rs
//! |-- tool-api/tests/registry.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/mutation_input.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_legacy.rs
//! |-- tools/src/files/replacers.rs
//! |-- tools/src/files/write.rs
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/input.rs
//! |-- tools/src/search/legacy.rs
//! |-- tools/src/shell/core.rs
//! |-- tools/src/shell/legacy.rs
//! |-- tools/tests/files_common.rs
//! |-- tools/tests/files_mutation/core_edit.rs
//! |-- tools/tests/files_mutation/legacy_edit.rs
//! |-- tools/tests/files_mutation/legacy_edit_lines.rs
//! |-- tools/tests/files_mutation/native.rs
//! |-- tools/tests/files_mutation/support.rs
//! |-- tools/tests/files_mutation/write.rs
//! |-- tools/tests/files_read/core.rs
//! |-- tools/tests/files_read/legacy.rs
//! |-- tools/tests/files_read/support.rs
//! |-- tools/tests/search_support.rs
//! |-- tools/tests/search_tools.rs
//! |-- tools/tests/shell_core.rs
//! `-- tools/tests/shell_legacy.rs
//! Invariants: wire values match @opencode-ai/llm ToolDefinition/ToolOutput/ToolResultValue JSON.
//! Contract tests: tool-api/tests/registry.rs

pub mod context;
pub mod definition;
pub mod output;
pub mod registry;

pub use context::{Authorizer, NoProgress, ToolContext, ToolProgress};
pub use definition::{Tool, ToolDefinition, ToolFailure, validate_name};
pub use output::{ToolContent, ToolOutput, ToolResultValue};
pub use registry::{
    BoundOutput, ExecuteInput, Materialization, OutputBounder, PassthroughBounder, Registration,
    RegistrationError, Settlement, ToolRegistry,
};
