//! Purpose: release in-flight attachment accounting even when a read future is cancelled.
//! Scope: internal RAII read lease, never performs file IO.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/attachments.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/attachments.rs
//! Invariants: each admitted read decrements accounting once, revoked tokens are never recreated.
//! Contract tests: desktop-host/tests/attachments_concurrency.rs

pub(super) struct ReadLease<'a> {
    pub(super) picker: &'a super::PickedFiles,
    pub(super) token: &'a str,
}

impl Drop for ReadLease<'_> {
    fn drop(&mut self) {
        let mut selections = self.picker.selections.lock();
        let Some(selection) = selections.get_mut(self.token) else {
            return;
        };
        selection.in_flight -= 1;
        if selection.paths.is_empty() && selection.in_flight == 0 {
            selections.remove(self.token);
        }
    }
}
