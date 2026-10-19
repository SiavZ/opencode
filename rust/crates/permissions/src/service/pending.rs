//! Purpose: tie pending permission cleanup to the entry inserted by one call.
//! Scope: in-memory permission entry ownership and cancellation cleanup.
//! Uses:
//! rust/crates/
//! `-- permissions/src/service.rs
//! External:
//! `-- futures::channel::oneshot
//! Used by:
//! rust/crates/
//! `-- permissions/src/service.rs
//! Invariants: a guard removes only its own entry, never a replacement with the same request ID.
//! Contract tests: permissions/tests/service.rs

use super::{PermissionError, PermissionRequest};
use futures::channel::oneshot;
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) type Settler = oneshot::Sender<Result<(), PermissionError>>;

pub(super) struct Pending {
    pub(super) request: PermissionRequest,
    pub(super) agent: Option<String>,
    pub(super) sender: Option<Settler>,
    pub(super) owner: Arc<()>,
}

pub(super) struct PendingGuard {
    pub(super) pending: Arc<Mutex<Vec<Pending>>>,
    pub(super) owner: Arc<()>,
    pub(super) armed: bool,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.armed {
            lock(&self.pending).retain(|item| !Arc::ptr_eq(&item.owner, &self.owner));
        }
    }
}

pub(super) fn lock(pending: &Mutex<Vec<Pending>>) -> MutexGuard<'_, Vec<Pending>> {
    pending.lock().unwrap_or_else(|poison| poison.into_inner())
}
