//! Purpose: test the updater controller state machine, coalescing, and error recovery.
//! Scope: unit tests for `UpdaterController`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! External:
//! |-- futures::future::BoxFuture
//! |-- parking_lot::Mutex
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover all TS `updater-controller.test.ts` cases.
//! Contract tests: desktop-host/tests/updater_controller.rs

use futures::future::BoxFuture;
use opencode_desktop_host::updater::{
    CheckResult, UpdaterBackend, UpdaterController, UpdaterError, UpdaterInput, UpdaterPersistence,
    UpdaterReadyRecord, UpdaterState,
};
use parking_lot::Mutex;
use std::sync::Arc;

struct MockBackend {
    check_calls: Arc<Mutex<usize>>,
    download_calls: Arc<Mutex<usize>>,
    install_calls: Arc<Mutex<usize>>,
    fail_install: bool,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            check_calls: Arc::new(Mutex::new(0)),
            download_calls: Arc::new(Mutex::new(0)),
            install_calls: Arc::new(Mutex::new(0)),
            fail_install: false,
        }
    }
}

impl UpdaterBackend for MockBackend {
    fn check_for_updates(&self) -> BoxFuture<'_, Result<Option<CheckResult>, UpdaterError>> {
        *self.check_calls.lock() += 1;
        Box::pin(async move {
            Ok(Some(CheckResult {
                is_update_available: true,
                version: Some("2.0.0".to_string()),
            }))
        })
    }

    fn download_update(&self) -> BoxFuture<'_, Result<(), UpdaterError>> {
        *self.download_calls.lock() += 1;
        Box::pin(async move { Ok(()) })
    }

    fn quit_and_install(&self) -> Result<(), UpdaterError> {
        *self.install_calls.lock() += 1;
        if self.fail_install {
            Err(UpdaterError::Backend("install failed".to_string()))
        } else {
            Ok(())
        }
    }
}

struct MockPersistence {
    ready: Arc<Mutex<Option<UpdaterReadyRecord>>>,
}

impl MockPersistence {
    fn new() -> Self {
        Self {
            ready: Arc::new(Mutex::new(None)),
        }
    }
    fn with_ready(version: &str) -> Self {
        Self {
            ready: Arc::new(Mutex::new(Some(UpdaterReadyRecord {
                version: version.to_string(),
            }))),
        }
    }
}

impl UpdaterPersistence for MockPersistence {
    fn get(&self) -> Result<Option<UpdaterReadyRecord>, UpdaterError> {
        Ok(self.ready.lock().clone())
    }

    fn set(&self, value: &UpdaterReadyRecord) -> Result<(), UpdaterError> {
        *self.ready.lock() = Some(value.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), UpdaterError> {
        *self.ready.lock() = None;
        Ok(())
    }
}

fn setup(
    current_version: &str,
    ready: Option<&str>,
) -> (UpdaterController, Arc<MockBackend>, Arc<MockPersistence>) {
    let backend = Arc::new(MockBackend::new());
    let persistence = Arc::new(if let Some(v) = ready {
        MockPersistence::with_ready(v)
    } else {
        MockPersistence::new()
    });
    let stop_calls = Arc::new(Mutex::new(0));
    let stop_calls_clone = stop_calls.clone();
    let controller = UpdaterController::new(UpdaterInput {
        enabled: true,
        current_version: current_version.to_string(),
        backend: backend.clone(),
        persistence: persistence.clone(),
        stop: Arc::new(move || {
            *stop_calls_clone.lock() += 1;
            Box::pin(async move { Ok(()) })
        }),
    });
    (controller, backend, persistence)
}

#[tokio::test]
async fn checks_downloads_persists_and_publishes_one_authoritative_ready_state() {
    let (controller, backend, persistence) = setup("1.0.0", None);
    let states = Arc::new(Mutex::new(Vec::new()));
    let states_clone = states.clone();
    let _disposer = controller.subscribe(move |state| {
        states_clone.lock().push(state.status().to_string());
    });

    controller.start().await;

    assert_eq!(*backend.check_calls.lock(), 1);
    assert_eq!(*backend.download_calls.lock(), 1);
    assert_eq!(persistence.ready.lock().as_ref().unwrap().version, "2.0.0");
    assert_eq!(
        *states.lock(),
        vec!["idle", "checking", "downloading", "ready"]
    );
    assert_eq!(
        controller.state(),
        UpdaterState::Ready {
            version: "2.0.0".to_string()
        }
    );
}

#[tokio::test]
async fn revalidates_persisted_target_through_updater_cache_on_launch() {
    let (controller, backend, _) = setup("1.0.0", Some("2.0.0"));
    controller.start().await;
    assert_eq!(*backend.check_calls.lock(), 1);
    assert_eq!(*backend.download_calls.lock(), 1);
    assert_eq!(
        controller.state(),
        UpdaterState::Ready {
            version: "2.0.0".to_string()
        }
    );
}

#[tokio::test]
async fn clears_target_already_installed_before_checking() {
    let (controller, backend, persistence) = setup("2.0.0", Some("2.0.0"));
    controller.start().await;
    assert!(persistence.ready.lock().is_none());
    assert_eq!(*backend.check_calls.lock(), 1);
    assert_eq!(*backend.download_calls.lock(), 0);
}

#[tokio::test]
async fn coalesces_concurrent_checks() {
    let (controller, backend, _) = setup("1.0.0", None);
    let (r1, r2, r3) = tokio::join!(controller.check(), controller.check(), controller.check());
    assert_eq!(*backend.check_calls.lock(), 1);
    assert_eq!(*backend.download_calls.lock(), 1);
    assert_eq!(r1.status(), "ready");
    assert_eq!(r2.status(), "ready");
    assert_eq!(r3.status(), "ready");
}

#[tokio::test]
async fn returns_to_ready_when_quit_and_install_returns_without_exiting() {
    let (controller, backend, _) = setup("1.0.0", None);
    controller.start().await;
    controller.install().await.unwrap();
    assert_eq!(*backend.check_calls.lock(), 1);
    assert_eq!(*backend.download_calls.lock(), 1);
    assert_eq!(*backend.install_calls.lock(), 1);
    assert_eq!(
        controller.state(),
        UpdaterState::Ready {
            version: "2.0.0".to_string()
        }
    );
}

#[tokio::test]
async fn returns_to_ready_when_installation_cannot_start() {
    let backend = Arc::new(MockBackend::new());
    let persistence = Arc::new(MockPersistence::new());
    let stop_fails = Arc::new(Mutex::new(false));
    let stop_fails_clone = stop_fails.clone();
    let controller = UpdaterController::new(UpdaterInput {
        enabled: true,
        current_version: "1.0.0".to_string(),
        backend: backend.clone(),
        persistence: persistence.clone(),
        stop: Arc::new(move || {
            let fails = *stop_fails_clone.lock();
            Box::pin(async move {
                if fails {
                    Err("stop failed".to_string())
                } else {
                    Ok(())
                }
            })
        }),
    });
    controller.start().await;
    *stop_fails.lock() = true;
    let err = controller.install().await.unwrap_err();
    assert!(matches!(err, UpdaterError::Stop(_)));
    assert_eq!(
        controller.state(),
        UpdaterState::Ready {
            version: "2.0.0".to_string()
        }
    );
}
