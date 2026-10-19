//! Purpose: crate root for the Session contract: Session-facing re-exports of schema values
//! (IDs, prompts, admission, messages, info), the unresolved prompt input, typed errors and the
//! service traits implemented by `session-runtime`. Holds no I/O and no execution logic.
//! Scope: pure values and object-safe async traits.
//! Uses:
//! rust/crates/
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/src/info.rs
//! |-- session-api/src/message.rs
//! |-- session-api/src/operations.rs
//! `-- session-api/src/prompt.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/commands/terminal.rs
//! `-- opencode/src/cli/commands/turn.rs
//! rust/crates/
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location.rs
//! |-- runtime/tests/process.rs
//! |-- runtime/tests/production.rs
//! |-- session-api/tests/wire.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/execution.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/publish.rs
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
//! |-- session-runtime/tests/compaction.rs
//! |-- session-runtime/tests/coordinator_execution.rs
//! |-- session-runtime/tests/runner_messages.rs
//! |-- session-runtime/tests/runner_turn.rs
//! |-- session-runtime/tests/runner_turn_steps.rs
//! |-- session-runtime/tests/runner_turn_tools.rs
//! |-- session-runtime/tests/support/provider.rs
//! |-- session-runtime/tests/support/publish.rs
//! |-- session-runtime/tests/support/sql.rs
//! `-- session-runtime/tests/todo.rs
//! Invariants: wire shapes are owned by `opencode-schema`; this crate adds Session semantics only.
//! Contract tests: session-api/tests/wire.rs

pub mod admission;
pub mod error;
pub mod ids;
pub mod info;
pub mod message;
pub mod operations;
pub mod prompt;

pub use admission::{Admitted, Delivery, PromptRequest};
pub use error::{SessionError, SessionResult};
pub use ids::{AgentID, LocationRef, MessageID, ModelRef, SessionID};
pub use info::SessionInfo;
pub use message::Message;
pub use prompt::{Prompt, PromptInput};
