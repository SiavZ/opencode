//! Purpose: shared gated-run builders for the BackgroundJobs contract tests. Not production logic.
//! Scope: test support only.
//! Uses:
//! rust/crates/
//! `-- session-runtime/src/background.rs
//! External:
//! |-- futures::future::FutureExt
//! `-- tokio::sync::oneshot
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/background.rs
//! `-- session-runtime/tests/background_control.rs
//! Invariants: helpers only build inputs and observe drops, they never inspect registry state.
//! Contract tests: session-runtime/tests/background.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::FutureExt;
use opencode_session_runtime::background::{JobRun, StartInput};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

pub fn input(id: Option<&str>, run: JobRun) -> StartInput {
    StartInput {
        id: id.map(str::to_owned),
        job_type: "test".into(),
        title: None,
        metadata: None,
        on_promote: None,
        on_interrupt: None,
        run,
    }
}

/// A run that resolves with the value sent on the returned gate.
pub fn gated() -> (oneshot::Sender<Result<String, String>>, JobRun) {
    let (tx, rx) = oneshot::channel();
    (
        tx,
        async move { rx.await.unwrap_or_else(|_| Err("gate dropped".into())) }.boxed(),
    )
}

pub fn ok(text: &str) -> JobRun {
    let text = text.to_owned();
    async move { Ok(text) }.boxed()
}

/// Sets the flag when dropped, proving the run future was torn down.
pub struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub fn never_with_flag() -> (Arc<AtomicBool>, JobRun) {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = DropFlag(dropped.clone());
    (
        dropped,
        async move {
            let _guard = guard;
            std::future::pending::<Result<String, String>>().await
        }
        .boxed(),
    )
}

pub fn metadata(value: Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}
