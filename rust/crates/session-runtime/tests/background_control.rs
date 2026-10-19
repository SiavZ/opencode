//! Purpose: verify BackgroundJobs promote/waitForPromotion/cancel/restart/shutdown against
//! packages/core/src/background-job.ts.
//! Scope: live tokio runtimes (current-thread, paused virtual time). No fixtures or mocks.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! `-- session-runtime/tests/background/helpers.rs
//! External:
//! |-- futures::future::FutureExt
//! |-- serde_json::json
//! |-- tokio::spawn
//! |-- tokio::task::yield_now
//! |-- tokio::test
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests observe behavior only through the public API and side effects of the submitted
//! runs and hooks. Expected values come from the baseline source and a recorded baseline probe
//! (interrupt-only error text, shutdown leaving status `running`).
//! Contract tests: session-runtime/tests/background.rs, session-runtime/tests/background_control.rs

#[path = "background/helpers.rs"]
mod helpers;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures::future::FutureExt;
use helpers::{gated, input, metadata, never_with_flag, ok};
use opencode_session_runtime::background::{BackgroundJobs, JobStatus, StartInput, WaitResult};
use serde_json::json;

#[tokio::test]
async fn promote_marks_background_runs_hook_once_and_wakes_waiters() {
    let jobs = BackgroundJobs::new();
    let hooks = Arc::new(AtomicUsize::new(0));
    let counter = hooks.clone();
    let (gate, run) = gated();
    let job = jobs.start(StartInput {
        metadata: metadata(json!({ "sessionId": "ses_1" })),
        on_promote: Some(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            .boxed(),
        ),
        ..input(None, run)
    });
    let waiter = {
        let (jobs, id) = (jobs.clone(), job.id.clone());
        tokio::spawn(async move { jobs.wait_for_promotion(&id).await })
    };
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    let promoted = jobs.promote(&job.id).await.unwrap();
    assert_eq!(
        promoted.metadata,
        metadata(json!({ "sessionId": "ses_1", "background": true }))
    );
    assert!(promoted.is_background());
    assert_eq!(hooks.load(Ordering::SeqCst), 1);
    assert_eq!(waiter.await.unwrap(), promoted);

    assert_eq!(jobs.promote(&job.id).await, Some(promoted.clone()));
    assert_eq!(hooks.load(Ordering::SeqCst), 1, "hook runs once");
    assert_eq!(jobs.wait_for_promotion(&job.id).await, promoted);

    gate.send(Ok("x".into())).unwrap();
    let done = jobs.wait(&job.id, None).await.info.unwrap();
    assert!(done.is_background());
    assert_eq!(
        jobs.promote(&job.id).await,
        None,
        "finished jobs are not promoted"
    );
    assert_eq!(jobs.promote("job_missing").await, None);
}

#[tokio::test]
async fn metadata_background_true_at_start_counts_as_promoted() {
    let jobs = BackgroundJobs::new();
    let hooks = Arc::new(AtomicUsize::new(0));
    let counter = hooks.clone();
    let (_gate, run) = gated();
    let job = jobs.start(StartInput {
        metadata: metadata(json!({ "background": true })),
        on_promote: Some(
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            .boxed(),
        ),
        ..input(None, run)
    });
    assert_eq!(jobs.wait_for_promotion(&job.id).await, job);
    assert_eq!(jobs.promote(&job.id).await, Some(job));
    assert_eq!(hooks.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn wait_for_promotion_never_resolves_for_unknown_or_finished_jobs() {
    let jobs = BackgroundJobs::new();
    let job = jobs.start(input(None, ok("x")));
    jobs.wait(&job.id, None).await;
    let finished = tokio::time::timeout(Duration::from_secs(60), jobs.wait_for_promotion(&job.id));
    assert!(finished.await.is_err());
    let unknown = tokio::time::timeout(Duration::from_secs(60), jobs.wait_for_promotion("job_x"));
    assert!(unknown.await.is_err());

    // A job that finishes while a promotion waiter is pending leaves the waiter pending.
    let (gate, run) = gated();
    let job = jobs.start(input(None, run));
    let waiter = {
        let (jobs, id) = (jobs.clone(), job.id.clone());
        tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(60), jobs.wait_for_promotion(&id)).await
        })
    };
    tokio::task::yield_now().await;
    gate.send(Ok("x".into())).unwrap();
    assert!(waiter.await.unwrap().is_err());
}

#[tokio::test]
async fn cancel_interrupts_joins_runs_and_settles_waiters() {
    let jobs = BackgroundJobs::new();
    let hooks = Arc::new(AtomicUsize::new(0));
    let (interrupt_count, promote_count) = (hooks.clone(), hooks.clone());
    let (dropped, run) = never_with_flag();
    let (queued_dropped, queued) = never_with_flag();
    let job = jobs.start(StartInput {
        on_interrupt: Some(
            async move {
                interrupt_count.fetch_add(10, Ordering::SeqCst);
            }
            .boxed(),
        ),
        on_promote: Some(
            async move {
                promote_count.fetch_add(1, Ordering::SeqCst);
            }
            .boxed(),
        ),
        ..input(None, run)
    });
    assert!(jobs.extend(&job.id, queued));
    let waiter = {
        let (jobs, id) = (jobs.clone(), job.id.clone());
        tokio::spawn(async move { jobs.wait(&id, None).await })
    };
    tokio::task::yield_now().await;

    let cancelled = jobs.cancel(&job.id).await.unwrap();
    // Joined: runs are torn down and the interrupt hook finished before cancel returned.
    assert!(dropped.load(Ordering::SeqCst));
    assert!(queued_dropped.load(Ordering::SeqCst));
    assert_eq!(hooks.load(Ordering::SeqCst), 10);
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.completed_at.is_some());
    assert_eq!(cancelled.error, None, "explicit cancel records no error");
    let waited = waiter.await.unwrap();
    assert_eq!(waited.info, Some(cancelled.clone()));
    assert!(!waited.timed_out);

    assert_eq!(jobs.cancel(&job.id).await, Some(cancelled.clone()));
    assert_eq!(jobs.promote(&job.id).await, None);
    assert_eq!(
        hooks.load(Ordering::SeqCst),
        10,
        "on_promote dropped by cancel"
    );
    assert_eq!(jobs.get(&job.id), Some(cancelled));
    assert_eq!(jobs.cancel("job_missing").await, None);
}

#[tokio::test]
async fn cancel_keeps_output_of_completed_extensions() {
    let jobs = BackgroundJobs::new();
    let job = jobs.start(input(None, ok("first")));
    let (_dropped, never) = never_with_flag();
    assert!(jobs.extend(&job.id, never));
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let cancelled = jobs.cancel(&job.id).await.unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert_eq!(cancelled.output.as_deref(), Some("first"));
}

#[tokio::test]
async fn restarting_a_finished_id_replaces_state_and_ignores_old_generation() {
    let jobs = BackgroundJobs::new();
    let (g1, r1) = gated();
    let first = jobs.start(StartInput {
        title: Some("old".into()),
        ..input(Some("job_restart"), r1)
    });
    // Queued old-generation run that only gets interrupted after the failure below.
    let (_old_dropped, old_queued) = never_with_flag();
    assert!(jobs.extend(&first.id, old_queued));
    g1.send(Err("boom".into())).unwrap();
    let failed = jobs.wait(&first.id, None).await.info.unwrap();
    assert_eq!(failed.status, JobStatus::Error);

    let (g2, r2) = gated();
    let restarted = jobs.start(StartInput {
        title: Some("new".into()),
        ..input(Some("job_restart"), r2)
    });
    assert_eq!(restarted.status, JobStatus::Running);
    assert_eq!(restarted.title.as_deref(), Some("new"));
    assert_eq!(restarted.error, None);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // The interrupted old-generation run settled after the restart without touching the new job.
    assert_eq!(jobs.get("job_restart").unwrap(), restarted);
    g2.send(Ok("fresh".into())).unwrap();
    let info = jobs.wait("job_restart", None).await.info.unwrap();
    assert_eq!(info.status, JobStatus::Completed);
    assert_eq!(info.output.as_deref(), Some("fresh"));
    assert_eq!(jobs.list().len(), 1);
}

#[tokio::test]
async fn shutdown_interrupts_and_joins_without_settling() {
    let jobs = BackgroundJobs::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    let (dropped, run) = never_with_flag();
    let job = jobs.start(StartInput {
        on_interrupt: Some(
            async move {
                flag.store(true, Ordering::SeqCst);
            }
            .boxed(),
        ),
        ..input(None, run)
    });
    let finished = jobs.start(input(None, ok("kept")));
    jobs.wait(&finished.id, None).await;
    let waiter = {
        let (jobs, id) = (jobs.clone(), job.id.clone());
        tokio::spawn(async move { jobs.wait(&id, None).await })
    };
    tokio::task::yield_now().await;

    jobs.shutdown().await;
    assert!(dropped.load(Ordering::SeqCst));
    assert!(interrupted.load(Ordering::SeqCst));
    // The abandoned in-memory registry is not a durable observation channel.
    assert_eq!(jobs.get(&job.id).unwrap().status, JobStatus::Running);
    assert_eq!(
        jobs.get(&finished.id).unwrap().output.as_deref(),
        Some("kept")
    );
    assert_eq!(
        waiter.await.unwrap(),
        WaitResult {
            info: None,
            timed_out: false
        }
    );
    // Cancel after shutdown settles without joining abandoned work.
    assert_eq!(
        jobs.cancel(&job.id).await.unwrap().status,
        JobStatus::Cancelled
    );
    // Registry stays usable after the owner scope closes, like the baseline.
    let after = jobs.start(input(None, ok("x")));
    let info = jobs.wait(&after.id, None).await.info.unwrap();
    assert_eq!(info.status, JobStatus::Completed);
}
