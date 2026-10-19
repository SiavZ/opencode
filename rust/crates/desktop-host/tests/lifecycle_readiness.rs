//! Purpose: contract tests for startup readiness, ported from `main/index.test.ts:5-37`.
//! Scope: fixture-free in-process tests (no platform or network).
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lifecycle/readiness.rs
//! External:
//! |-- serde_json::to_value
//! |-- tokio::spawn
//! |-- tokio::task::yield_now
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: every waiter resolves on success and failure.
//! Contract tests: this file

use opencode_desktop_host::lifecycle::readiness::{
    InitializationError, Readiness, ServerReadyData,
};

fn ready() -> ServerReadyData {
    ServerReadyData {
        url: "http://127.0.0.1:4096".into(),
        username: Some("opencode".into()),
        password: None,
    }
}

// index.test.ts:13 "forwards loading task failures before renderer initialization"
#[tokio::test]
async fn forwards_loading_task_failures_before_renderer_initialization() {
    let initialization = Readiness::<ServerReadyData>::new();
    let result = initialization
        .forward_failure(async { Err::<(), _>("sidecar startup failed") })
        .await;
    assert_eq!(
        result,
        Err(InitializationError::Failed("sidecar startup failed".into()))
    );
    assert_eq!(
        initialization.wait().await,
        Err(InitializationError::Failed("sidecar startup failed".into()))
    );
}

// index.test.ts:25 "forwards loading task failures while renderer initialization waits"
#[tokio::test]
async fn forwards_loading_task_failures_while_renderer_initialization_waits() {
    let initialization = Readiness::<ServerReadyData>::new();
    let waiting = tokio::spawn({
        let initialization = initialization.clone();
        async move { initialization.wait().await }
    });
    tokio::task::yield_now().await;
    let _ = initialization
        .forward_failure(async { Err::<(), _>("sidecar startup failed") })
        .await;
    assert_eq!(
        waiting.await.expect("join"),
        Err(InitializationError::Failed("sidecar startup failed".into()))
    );
}

#[tokio::test]
async fn defects_are_forwarded_like_effect_die() {
    let initialization = Readiness::<ServerReadyData>::new();
    let result = initialization
        .forward_failure(async {
            if true {
                panic!("sidecar startup failed");
            }
            Ok::<(), String>(())
        })
        .await;
    let expected = InitializationError::Panicked("sidecar startup failed".into());
    assert_eq!(result, Err(expected.clone()));
    assert_eq!(initialization.wait().await, Err(expected));
}

#[tokio::test]
async fn all_waiters_resolve_on_success_and_late_waiters_see_stored_value() {
    let initialization = Readiness::<ServerReadyData>::new();
    let waiters: Vec<_> = (0..4)
        .map(|_| {
            let initialization = initialization.clone();
            tokio::spawn(async move { initialization.wait().await })
        })
        .collect();
    tokio::task::yield_now().await;
    let ran = initialization
        .forward_failure(async {
            assert!(initialization.succeed(ready()));
            // Work after readiness (the health wait) still runs inside the forwarder.
            Ok::<_, String>(7)
        })
        .await;
    assert_eq!(ran, Ok(7));
    for waiter in waiters {
        assert_eq!(waiter.await.expect("join"), Ok(ready()));
    }
    assert_eq!(initialization.wait().await, Ok(ready()));
}

#[tokio::test]
async fn failure_after_success_does_not_override_readiness() {
    let initialization = Readiness::<ServerReadyData>::new();
    let result = initialization
        .forward_failure(async {
            initialization.succeed(ready());
            Err::<(), _>("health check failed")
        })
        .await;
    assert!(result.is_err());
    assert_eq!(initialization.wait().await, Ok(ready()));
    assert!(!initialization.fail(InitializationError::Cancelled));
    assert!(!initialization.succeed(ready()));
}

#[tokio::test]
async fn dropping_the_loading_task_fails_waiters_instead_of_hanging() {
    let initialization = Readiness::<ServerReadyData>::new();
    let task = tokio::spawn({
        let initialization = initialization.clone();
        async move {
            initialization
                .forward_failure(std::future::pending::<Result<(), String>>())
                .await
        }
    });
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;
    assert_eq!(
        initialization.wait().await,
        Err(InitializationError::Cancelled)
    );
}

#[test]
fn server_ready_data_wire_shape_keeps_nulls() {
    let value = serde_json::to_value(ServerReadyData {
        url: "http://x".into(),
        username: None,
        password: None,
    })
    .expect("serialize");
    assert_eq!(
        value.to_string(),
        r#"{"url":"http://x","username":null,"password":null}"#
    );
}
