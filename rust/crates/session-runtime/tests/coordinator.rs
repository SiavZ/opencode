//! Purpose: port every case of packages/core/test/session-run-coordinator.test.ts against the real
//! `RunCoordinator`. Deterministic: gates are latches, no sleeps decide outcomes.
//! Scope: integration test (live implementation, no fixtures).
//! Uses:
//! rust/crates/
//! `-- session-runtime/src/coordinator.rs
//! External:
//! |-- futures::future::FutureExt
//! |-- tokio::spawn
//! |-- tokio::sync::watch
//! |-- tokio::task::yield_now
//! |-- tokio::test
//! |-- tokio_util::sync::CancellationToken
//! |-- watch::Sender
//! `-- watch::channel
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test owns its coordinator and shuts it down or lets every drain settle.
//! Contract tests: session-runtime/tests/coordinator.rs

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::FutureExt;
use opencode_session_runtime::coordinator::{RunCoordinator, RunError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// One-shot latch mirroring Effect `Deferred<void>`.
#[derive(Clone)]
struct Latch(watch::Sender<bool>);

impl Latch {
    fn new() -> Self {
        Self(watch::channel(false).0)
    }
    fn open(&self) {
        self.0.send_replace(true);
    }
    async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        receiver
            .wait_for(|open| *open)
            .await
            .expect("latch sender alive");
    }
}

type Coordinator<E = String> = RunCoordinator<String, E>;

fn key(value: &str) -> String {
    value.to_owned()
}

fn sorted(coordinator: &Coordinator<impl Send + Sync + 'static>) -> Vec<String> {
    let mut keys: Vec<_> = coordinator.active().into_iter().collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn joins_concurrent_resumes_for_one_key() {
    let gate = Latch::new();
    let started = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let (gate, started, runs) = (gate.clone(), started.clone(), runs.clone());
        move |_, _, _| {
            let (gate, started, runs) = (gate.clone(), started.clone(), runs.clone());
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                started.open();
                gate.wait().await;
                Ok(())
            }
            .boxed()
        }
    });

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    started.wait().await;
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    tokio::task::yield_now().await;

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    gate.open();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn joins_a_wake_started_execution_without_forcing_a_successor() {
    let started = Latch::new();
    let gate = Latch::new();
    let forces = Arc::new(Mutex::new(Vec::new()));
    let coordinator: Coordinator = RunCoordinator::new({
        let (started, gate, forces) = (started.clone(), gate.clone(), forces.clone());
        move |_, force, _| {
            let (started, gate, forces) = (started.clone(), gate.clone(), forces.clone());
            async move {
                forces.lock().unwrap().push(force);
                started.open();
                gate.wait().await;
                Ok(())
            }
            .boxed()
        }
    });

    coordinator.wake(key("session"));
    started.wait().await;
    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    tokio::task::yield_now().await;
    gate.open();
    resumed.await.unwrap().unwrap();

    assert_eq!(*forces.lock().unwrap(), vec![false]);
}

#[tokio::test]
async fn starts_execution_when_woken_while_idle() {
    let drained = Latch::new();
    let coordinator: Coordinator = RunCoordinator::new({
        let drained = drained.clone();
        move |_, _, _| {
            drained.open();
            async { Ok(()) }.boxed()
        }
    });

    coordinator.wake(key("session"));
    drained.wait().await;
}

#[tokio::test]
async fn snapshots_only_active_executions() {
    let first_started = Latch::new();
    let second_started = Latch::new();
    let first_gate = Latch::new();
    let second_gate = Latch::new();
    let coordinator: Coordinator = RunCoordinator::new({
        let latches = (
            first_started.clone(),
            second_started.clone(),
            first_gate.clone(),
            second_gate.clone(),
        );
        move |key: String, _, _| {
            let (started, gate) = if key == "first" {
                (latches.0.clone(), latches.2.clone())
            } else {
                (latches.1.clone(), latches.3.clone())
            };
            async move {
                started.open();
                gate.wait().await;
                Ok(())
            }
            .boxed()
        }
    });

    assert!(sorted(&coordinator).is_empty());
    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("first")).await }
    });
    first_started.wait().await;
    assert_eq!(sorted(&coordinator), vec!["first"]);

    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("second")).await }
    });
    second_started.wait().await;
    assert_eq!(sorted(&coordinator), vec!["first", "second"]);

    first_gate.open();
    first.await.unwrap().unwrap();
    assert_eq!(sorted(&coordinator), vec!["second"]);
    second_gate.open();
    second.await.unwrap().unwrap();
    assert!(sorted(&coordinator).is_empty());
}

#[tokio::test]
async fn cleans_active_executions_after_failure_and_defect() {
    let coordinator: Coordinator = RunCoordinator::new(|key: String, _, _| {
        async move {
            if key == "failure" {
                return Err("failed".to_owned());
            }
            panic!("defect");
        }
        .boxed()
    });

    let failed = coordinator.run(key("failure")).await;
    assert!(matches!(&failed, Err(RunError::Failed(error)) if error.as_str() == "failed"));
    assert!(sorted(&coordinator).is_empty());

    let died = coordinator.run(key("defect")).await;
    assert!(matches!(&died, Err(RunError::Panicked(message)) if &**message == "defect"));
    assert!(sorted(&coordinator).is_empty());
}

#[tokio::test]
async fn cleans_active_executions_when_its_scope_closes() {
    let started = Latch::new();
    let coordinator: Coordinator = RunCoordinator::new({
        let started = started.clone();
        move |_, _, cancel: CancellationToken| {
            let started = started.clone();
            async move {
                started.open();
                // Effect.never, ended only by interruption.
                cancel.cancelled().await;
                Ok(())
            }
            .boxed()
        }
    });
    coordinator.wake(key("session"));
    started.wait().await;
    assert_eq!(sorted(&coordinator), vec!["session"]);

    coordinator.shutdown().await;

    assert!(sorted(&coordinator).is_empty());
    // A closed coordinator refuses new work.
    assert!(matches!(
        coordinator.run(key("session")).await,
        Err(RunError::Interrupted)
    ));
    coordinator.wake(key("session"));
    assert!(sorted(&coordinator).is_empty());
}

#[tokio::test]
async fn coalesces_wakes_received_during_active_execution() {
    let first_started = Latch::new();
    let first_gate = Latch::new();
    let second_started = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let latches = (
            first_started.clone(),
            first_gate.clone(),
            second_started.clone(),
        );
        let runs = runs.clone();
        move |_, _, _| {
            let (first_started, first_gate, second_started) = latches.clone();
            let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if run == 1 {
                    first_started.open();
                    first_gate.wait().await;
                } else {
                    second_started.open();
                }
                Ok(())
            }
            .boxed()
        }
    });

    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    first_started.wait().await;
    let wakes: Vec<_> = (0..3)
        .map(|_| {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.wake(key("session")) })
        })
        .collect();
    for wake in wakes {
        wake.await.unwrap();
    }
    first_gate.open();
    second_started.wait().await;
    resumed.await.unwrap().unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runs_again_when_woken_during_the_follow_up() {
    let first_started = Latch::new();
    let first_gate = Latch::new();
    let second_started = Latch::new();
    let second_gate = Latch::new();
    let third_started = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let latches = (
            first_started.clone(),
            first_gate.clone(),
            second_started.clone(),
            second_gate.clone(),
            third_started.clone(),
        );
        let runs = runs.clone();
        move |_, _, _| {
            let latches = latches.clone();
            let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                match run {
                    1 => {
                        latches.0.open();
                        latches.1.wait().await;
                    }
                    2 => {
                        latches.2.open();
                        latches.3.wait().await;
                    }
                    _ => latches.4.open(),
                }
                Ok(())
            }
            .boxed()
        }
    });

    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    first_started.wait().await;
    coordinator.wake(key("session"));
    first_gate.open();
    second_started.wait().await;
    coordinator.wake(key("session"));
    second_gate.open();
    third_started.wait().await;
    resumed.await.unwrap().unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn does_nothing_when_interrupted_while_idle() {
    let coordinator: Coordinator = RunCoordinator::new(|_, _, _| async { Ok(()) }.boxed());
    coordinator.interrupt(key("session")).await;
    assert!(sorted(&coordinator).is_empty());
}

#[tokio::test]
async fn interrupts_active_execution_and_clears_its_pending_wake() {
    let started = Latch::new();
    let interrupted = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let (started, interrupted, runs) = (started.clone(), interrupted.clone(), runs.clone());
        move |_, _, cancel: CancellationToken| {
            let (started, interrupted) = (started.clone(), interrupted.clone());
            runs.fetch_add(1, Ordering::SeqCst);
            async move {
                started.open();
                cancel.cancelled().await;
                interrupted.open();
                Ok(())
            }
            .boxed()
        }
    });

    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    started.wait().await;
    coordinator.wake(key("session"));
    coordinator.interrupt(key("session")).await;
    interrupted.wait().await;

    assert!(matches!(resumed.await.unwrap(), Err(RunError::Interrupted)));
    assert!(sorted(&coordinator).is_empty());
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// Drain whose first run blocks until interrupted, then runs async cleanup gated by `cleanup_gate`.
fn interrupt_cleanup_drain(
    first_started: Latch,
    cleanup_started: Latch,
    cleanup_gate: Latch,
    second_started: Latch,
    forces: Arc<Mutex<Vec<bool>>>,
) -> Coordinator {
    RunCoordinator::new(move |_, force, cancel: CancellationToken| {
        let run = {
            let mut forces = forces.lock().unwrap();
            forces.push(force);
            forces.len()
        };
        let latches = (
            first_started.clone(),
            cleanup_started.clone(),
            cleanup_gate.clone(),
            second_started.clone(),
        );
        async move {
            if run == 1 {
                latches.0.open();
                cancel.cancelled().await;
                latches.1.open();
                latches.2.wait().await;
                return Ok(());
            }
            latches.3.open();
            Ok(())
        }
        .boxed()
    })
}

#[tokio::test]
async fn runs_a_wake_registered_during_interruption_cleanup() {
    let (first_started, cleanup_started, cleanup_gate, second_started) =
        (Latch::new(), Latch::new(), Latch::new(), Latch::new());
    let forces = Arc::new(Mutex::new(Vec::new()));
    let coordinator = interrupt_cleanup_drain(
        first_started.clone(),
        cleanup_started.clone(),
        cleanup_gate.clone(),
        second_started.clone(),
        forces.clone(),
    );

    coordinator.wake(key("session"));
    first_started.wait().await;
    let interrupt = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.interrupt(key("session")).await }
    });
    cleanup_started.wait().await;
    coordinator.wake(key("session"));
    cleanup_gate.open();
    interrupt.await.unwrap();
    second_started.wait().await;

    assert_eq!(forces.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn starts_a_resume_registered_during_interruption_cleanup() {
    let (first_started, cleanup_started, cleanup_gate, second_started) =
        (Latch::new(), Latch::new(), Latch::new(), Latch::new());
    let forces = Arc::new(Mutex::new(Vec::new()));
    let coordinator = interrupt_cleanup_drain(
        first_started.clone(),
        cleanup_started.clone(),
        cleanup_gate.clone(),
        second_started.clone(),
        forces.clone(),
    );

    coordinator.wake(key("session"));
    first_started.wait().await;
    let interrupt = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.interrupt(key("session")).await }
    });
    cleanup_started.wait().await;
    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    tokio::task::yield_now().await;
    cleanup_gate.open();
    interrupt.await.unwrap();
    resumed.await.unwrap().unwrap();
    second_started.wait().await;

    assert_eq!(*forces.lock().unwrap(), vec![false, true]);
}

#[tokio::test]
async fn starts_one_follow_up_when_a_wake_races_with_failure() {
    let started = Latch::new();
    let gate = Latch::new();
    let second_started = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let latches = (started.clone(), gate.clone(), second_started.clone());
        let runs = runs.clone();
        move |_, _, _| {
            let latches = latches.clone();
            let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if run == 1 {
                    latches.0.open();
                    latches.1.wait().await;
                    return Err("failed".to_owned());
                }
                latches.2.open();
                Ok(())
            }
            .boxed()
        }
    });

    let resumed = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    started.wait().await;
    coordinator.wake(key("session"));
    gate.open();

    assert!(
        matches!(resumed.await.unwrap(), Err(RunError::Failed(error)) if error.as_str() == "failed")
    );
    second_started.wait().await;
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn does_not_cancel_execution_when_a_joined_waiter_is_interrupted() {
    let started = Latch::new();
    let gate = Latch::new();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let runs = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let (started, gate, runs, cancelled) = (
            started.clone(),
            gate.clone(),
            runs.clone(),
            cancelled.clone(),
        );
        move |_, _, cancel: CancellationToken| {
            let (started, gate, cancelled) = (started.clone(), gate.clone(), cancelled.clone());
            runs.fetch_add(1, Ordering::SeqCst);
            async move {
                started.open();
                gate.wait().await;
                if cancel.is_cancelled() {
                    cancelled.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
            .boxed()
        }
    });

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    started.wait().await;
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    tokio::task::yield_now().await;
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    gate.open();
    first.await.unwrap().unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(cancelled.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dropping_the_starting_waiter_does_not_cancel_execution() {
    let started = Latch::new();
    let gate = Latch::new();
    let finished = Latch::new();
    let coordinator: Coordinator = RunCoordinator::new({
        let (started, gate, finished) = (started.clone(), gate.clone(), finished.clone());
        move |_, _, _| {
            let (started, gate, finished) = (started.clone(), gate.clone(), finished.clone());
            async move {
                started.open();
                gate.wait().await;
                finished.open();
                Ok(())
            }
            .boxed()
        }
    });

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("session")).await }
    });
    started.wait().await;
    first.abort();
    let _ = first.await;
    assert_eq!(sorted(&coordinator), vec!["session"]);
    gate.open();
    finished.wait().await;
    coordinator.shutdown().await;
    assert!(sorted(&coordinator).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_different_keys_concurrently() {
    let gate = Latch::new();
    let both_started = Latch::new();
    let active = Arc::new(AtomicUsize::new(0));
    let coordinator: Coordinator = RunCoordinator::new({
        let (gate, both_started, active) = (gate.clone(), both_started.clone(), active.clone());
        move |_, _, _| {
            let (gate, both_started) = (gate.clone(), both_started.clone());
            if active.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                both_started.open();
            }
            async move {
                gate.wait().await;
                Ok(())
            }
            .boxed()
        }
    });

    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("first")).await }
    });
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.run(key("second")).await }
    });
    both_started.wait().await;
    gate.open();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
}

#[tokio::test]
async fn trampolines_synchronous_self_waking_execution() {
    const LIMIT: usize = 20_000;
    let completed = Latch::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let slot: Arc<Mutex<Option<Coordinator<std::convert::Infallible>>>> =
        Arc::new(Mutex::new(None));
    let coordinator: Coordinator<std::convert::Infallible> = RunCoordinator::new({
        let (completed, runs, slot) = (completed.clone(), runs.clone(), slot.clone());
        move |key: String, _, _| {
            let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
            if run < LIMIT {
                if let Some(coordinator) = slot.lock().unwrap().as_ref() {
                    coordinator.wake(key);
                }
            } else {
                completed.open();
            }
            async { Ok(()) }.boxed()
        }
    });
    *slot.lock().unwrap() = Some(coordinator.clone());

    coordinator.wake(key("session"));
    completed.wait().await;

    assert_eq!(runs.load(Ordering::SeqCst), LIMIT);
    slot.lock().unwrap().take();
}
