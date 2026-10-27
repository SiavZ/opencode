//! Purpose: own process-wide models inventory refresh and forward refresh notifications to the real event store.
//! Scope: one cancellable and joined background task per Process.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! `-- provider-adapters/src/catalog/refresh.rs
//! External:
//! |-- tokio::select
//! `-- tokio_util::task::TaskTracker
//! Used by:
//! rust/crates/
//! `-- runtime/src/process.rs
//! Invariants: no ambient environment or detached refresh task; disabling downloads suppresses periodic fetching only.
//! Contract tests: runtime/tests/model_refresh.rs

use opencode_event_api::{Definition, EventStore, PublishOptions};
use opencode_provider_adapters::catalog::ModelsService;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) struct ProcessTasks {
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl ProcessTasks {
    pub(crate) fn start(
        models: Arc<ModelsService>,
        events: Arc<dyn EventStore>,
        periodic: bool,
    ) -> Self {
        let cancel = CancellationToken::new();
        let tasks = TaskTracker::new();
        let mut refreshed = models.subscribe();
        let stop = cancel.clone();
        tasks.spawn(async move {
            let next = tokio::time::sleep(Duration::ZERO);
            tokio::pin!(next);
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    notification = refreshed.recv() => match notification {
                        Ok(()) => {
                            let definition = Definition::ephemeral("models-dev.refreshed");
                            let publication = events.publish(&definition, serde_json::json!({}), PublishOptions::default());
                            tokio::select! {
                                biased;
                                () = stop.cancelled() => break,
                                result = publication => if let Err(error) = result { tracing::warn!(%error, "models refresh notification failed"); },
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => tracing::warn!(skipped, "models refresh notifications lagged"),
                    },
                    () = &mut next, if periodic => {
                        tokio::select! {
                            biased;
                            () = stop.cancelled() => break,
                            result = models.refresh(false) => if let Err(error) = result { tracing::warn!(%error, "periodic models refresh failed"); },
                        }
                        next.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(60 * 60));
                    }
                }
            }
        });
        Self { cancel, tasks }
    }

    pub(crate) async fn shutdown(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Drop for ProcessTasks {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
