//! Purpose: provider-turn runner: one llm.stream per turn, history reload, steer/queue promotion
//! (owner p04.execution). Submodules are owned by p04.execution children.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/runner/max_steps.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/runner/to_llm_message.rs
//! `-- session-runtime/src/runner/turn.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/execution.rs
//! `-- session-runtime/src/lib.rs
//! Invariants: module list owned by p04.execution; children edit only their own submodule.
//! The `SessionRunner` trait below is the binding seam between execution.rs and runner/turn.rs.
//! Contract tests: session-runtime/tests/runner*.rs

pub mod max_steps;
pub mod model;
pub mod ports;
pub mod publish;
pub mod to_llm_message;
pub mod turn;

use async_trait::async_trait;
use opencode_session_api::{SessionError, SessionID};
use tokio_util::sync::CancellationToken;

/// `SessionRunner.Interface`: runs one local continuation from already-recorded Session history.
#[async_trait]
pub trait SessionRunner: Send + Sync {
    /// Drains eligible durable work. `force=true` (explicit resume) performs one provider attempt
    /// even when no input is eligible. `force=false` (advisory wake) exits without pending input.
    async fn run(&self, session_id: &SessionID, force: bool) -> Result<(), SessionError>;

    /// `run` under the coordinator's interruption token. The default drops `run` when the token
    /// fires; runners that must settle durable state on interruption (the LLM runner fails
    /// unsettled tools and the active assistant) override it and observe the token themselves.
    async fn run_cancellable(
        &self,
        session_id: &SessionID,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), SessionError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(SessionError::Cancelled),
            result = self.run(session_id, force) => result,
        }
    }
}
