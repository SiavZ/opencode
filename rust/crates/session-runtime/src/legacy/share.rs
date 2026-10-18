//! Purpose: `SessionShare.share/unshare/create` (packages/opencode/src/share/session.ts:26-46):
//! config gate, remote share through the `ShareRemote` port (hosted `ShareNext`), and the
//! `session.updated` share URL write.
//! Scope: one Session.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/legacy_prompt.rs
//! Invariants: `share` fails with "Sharing is disabled in configuration" before any remote call when
//! `share = "disabled"`; the URL is persisted only after the remote create succeeded; `unshare`
//! removes remotely first, then clears the URL. `auto_share` targets only root Sessions.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use super::LegacySession;
use super::store::SessionInfo;
use super::types::{LegacyError, str_field};

impl LegacySession {
    /// `SessionShare.share`.
    pub async fn share(&self, session: &str) -> Result<String, LegacyError> {
        if self.deps.config.share.as_deref() == Some("disabled") {
            return Err(LegacyError::Unknown(
                "Sharing is disabled in configuration".into(),
            ));
        }
        let url = self.deps.share.create(session).await?;
        self.deps.store.set_share(session, Some(&url)).await?;
        Ok(url)
    }

    /// `SessionShare.unshare`.
    pub async fn unshare(&self, session: &str) -> Result<(), LegacyError> {
        self.deps.share.remove(session).await?;
        self.deps.store.set_share(session, None).await
    }

    /// `SessionShare.create` auto-share tail: after a Session was created, share it in the
    /// background when `share = "auto"` or the auto-share flag is set. Failures are ignored, like
    /// the baseline `Effect.ignore`. Returns the spawned task so the owner can join it.
    pub fn auto_share(&self, info: &SessionInfo) -> Option<tokio::task::JoinHandle<()>> {
        if info.get("parentID").is_some_and(|parent| !parent.is_null()) {
            return None;
        }
        if !(self.deps.config.auto_share || self.deps.config.share.as_deref() == Some("auto")) {
            return None;
        }
        let this = self.clone();
        let id = str_field(info, "id").unwrap_or_default().to_owned();
        Some(tokio::spawn(async move {
            let _ = this.share(&id).await;
        }))
    }
}
