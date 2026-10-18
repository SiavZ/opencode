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
//! `-- none, reserved contract (owner: phase-05, consumers tools, session-runtime, server, codemode, integration-api)
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
