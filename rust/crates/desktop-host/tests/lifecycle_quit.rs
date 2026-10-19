//! Purpose: contract tests for quit/relaunch coordination (`main/index.ts:167-177,224-251`).
//! Scope: in-process hooks with paused tokio time.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lifecycle/quit.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- tokio::spawn
//! |-- tokio::task::yield_now
//! |-- tokio::test
//! `-- tokio::time::sleep
//! Used by:
//! `-- none, entrypoint
//! Invariants: failed or cancelled attempts never strand the quitting flag.
//! Contract tests: this file

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opencode_desktop_host::lifecycle::quit::*;
use parking_lot::Mutex;

const T: Duration = Duration::from_secs(5);

#[tokio::test(start_paused = true)]
async fn phases_run_in_order_and_hooks_are_joined() {
    let quit = QuitCoordinator::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    for (name, phase, delay) in [
        ("will", QuitPhase::WillQuit, 0),
        ("before-slow", QuitPhase::BeforeQuit, 100),
        ("before-fast", QuitPhase::BeforeQuit, 1),
    ] {
        let log = Arc::clone(&log);
        quit.register(name, phase, HookPolicy::BestEffort, T, move |_| {
            let log = Arc::clone(&log);
            async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                log.lock().push(name);
                Ok(())
            }
        });
    }
    assert!(!quit.is_quitting());
    let decision = quit.request(QuitReason::User).await;
    assert_eq!(
        decision,
        QuitDecision::Exit {
            relaunch: false,
            best_effort_failures: vec![]
        }
    );
    assert_eq!(*log.lock(), vec!["before-fast", "before-slow", "will"]);
    assert!(quit.is_quitting());
    // After deciding to exit, further requests return the same decision without rerunning.
    assert_eq!(quit.request(QuitReason::Signal).await, decision);
    assert_eq!(log.lock().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn best_effort_failure_and_timeout_still_exit() {
    let quit = QuitCoordinator::new();
    quit.register(
        "stop-sidecars",
        QuitPhase::BeforeQuit,
        HookPolicy::BestEffort,
        T,
        |_| async { Err("kill failed".to_string()) },
    );
    quit.register(
        "slow",
        QuitPhase::WillQuit,
        HookPolicy::BestEffort,
        Duration::from_millis(10),
        |_| async {
            std::future::pending::<()>().await;
            Ok(())
        },
    );
    let QuitDecision::Exit {
        best_effort_failures,
        ..
    } = quit.request(QuitReason::Signal).await
    else {
        panic!("expected exit");
    };
    assert_eq!(best_effort_failures.len(), 2);
    assert!(matches!(best_effort_failures[0], HookError::Failed { .. }));
    assert!(matches!(
        best_effort_failures[1],
        HookError::TimedOut { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn required_failure_aborts_and_restores_running_then_retry_succeeds() {
    let quit = QuitCoordinator::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let later = Arc::new(AtomicUsize::new(0));
    {
        let attempts = Arc::clone(&attempts);
        quit.register(
            "flush-drafts",
            QuitPhase::BeforeQuit,
            HookPolicy::Required,
            T,
            move |_| {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        Err("disk full".into())
                    } else {
                        Ok(())
                    }
                }
            },
        );
    }
    {
        let later = Arc::clone(&later);
        quit.register(
            "will",
            QuitPhase::WillQuit,
            HookPolicy::BestEffort,
            T,
            move |_| {
                later.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        );
    }
    let decision = quit.request(QuitReason::Relaunch).await;
    assert!(matches!(
        decision,
        QuitDecision::Aborted {
            error: HookError::Failed { .. }
        }
    ));
    assert!(!quit.is_quitting(), "quitting flag must be restored");
    assert_eq!(later.load(Ordering::SeqCst), 0, "later phase must not run");

    // Relaunch flag does not leak from the aborted attempt.
    assert_eq!(
        quit.request(QuitReason::User).await,
        QuitDecision::Exit {
            relaunch: false,
            best_effort_failures: vec![]
        }
    );
}

#[tokio::test(start_paused = true)]
async fn cancel_restores_state_and_cancels_hook_tokens() {
    let quit = QuitCoordinator::new();
    let observed = Arc::new(AtomicUsize::new(0));
    {
        let observed = Arc::clone(&observed);
        quit.register(
            "wait",
            QuitPhase::BeforeQuit,
            HookPolicy::Required,
            T,
            move |token| {
                let observed = Arc::clone(&observed);
                async move {
                    token.cancelled().await;
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        );
    }
    let mut flag = quit.subscribe_quitting();
    let task = tokio::spawn({
        let quit = quit.clone();
        async move { quit.request(QuitReason::User).await }
    });
    flag.wait_for(|q| *q).await.expect("quitting");
    assert!(quit.cancel());
    assert_eq!(task.await.expect("join"), QuitDecision::Cancelled);
    assert!(!quit.is_quitting());
    assert!(!quit.cancel(), "no active attempt");
}

#[tokio::test(start_paused = true)]
async fn concurrent_requests_join_and_relaunch_is_sticky() {
    let quit = QuitCoordinator::new();
    let runs = Arc::new(AtomicUsize::new(0));
    {
        let runs = Arc::clone(&runs);
        quit.register(
            "slow",
            QuitPhase::BeforeQuit,
            HookPolicy::BestEffort,
            T,
            move |_| {
                runs.fetch_add(1, Ordering::SeqCst);
                async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(())
                }
            },
        );
    }
    let mut flag = quit.subscribe_quitting();
    let first = tokio::spawn({
        let quit = quit.clone();
        async move { quit.request(QuitReason::User).await }
    });
    flag.wait_for(|q| *q).await.expect("quitting");
    let second = quit.relaunch().await;
    let first = first.await.expect("join");
    let expected = QuitDecision::Exit {
        relaunch: true,
        best_effort_failures: vec![],
    };
    assert_eq!(first, expected);
    assert_eq!(second, expected);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn dropped_driver_publishes_cancelled_to_joiners() {
    let quit = QuitCoordinator::new();
    quit.register(
        "hang",
        QuitPhase::BeforeQuit,
        HookPolicy::Required,
        Duration::from_secs(3600),
        |_| async {
            std::future::pending::<()>().await;
            Ok(())
        },
    );
    let mut flag = quit.subscribe_quitting();
    let driver = tokio::spawn({
        let quit = quit.clone();
        async move { quit.request(QuitReason::User).await }
    });
    flag.wait_for(|q| *q).await.expect("quitting");
    let joiner = tokio::spawn({
        let quit = quit.clone();
        async move { quit.request(QuitReason::Signal).await }
    });
    tokio::task::yield_now().await;
    driver.abort();
    let _ = driver.await;
    assert_eq!(joiner.await.expect("join"), QuitDecision::Cancelled);
    assert!(!quit.is_quitting());
}
