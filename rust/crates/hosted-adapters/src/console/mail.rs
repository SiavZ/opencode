//! Purpose: console invite mail rendering (port of `packages/console/mail/emails/templates/
//! InviteEmail.tsx`) and SES v2 SigV4 delivery (port of `packages/console/core/src/aws.ts`).
//! Scope: stub created by the phase-12 parent, filled by child node p12.ops.
//! Uses:
//! `-- none
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/mod.rs
//! Invariants: owned by p12.ops. The `InviteMailer` port below is the frozen seam that
//! p12.identity calls from the invite flow; keep its signature.
//! Contract tests: none yet.

/// Data needed to render and send `InviteEmail` (see `packages/console/core/src/user.ts` invite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteMail {
    pub to: String,
    pub inviter: String,
    pub workspace_id: String,
    pub workspace_name: String,
}

/// Port used by the membership invite flow. The baseline swallows send errors after the
/// membership row is persisted, so callers log and ignore `Err`.
#[async_trait::async_trait]
pub trait InviteMailer: Send + Sync + std::fmt::Debug {
    async fn send_invite(&self, mail: &InviteMail) -> Result<(), String>;
}
