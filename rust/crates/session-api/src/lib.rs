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
//! rust/crates/
//! `-- session-runtime/src/lib.rs
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
