//! Purpose: `LocalSessionExecution` behavior (packages/core/src/session/execution/local.ts plus the
//! execution cases of session-run-coordinator.test.ts): placement per drain, Location routing,
//! resume join, wake coalescing, idle interrupt and missing Session.
//! Scope: integration test. Fixture-based: the store and per-Location runners are in-memory
//! recording fixtures; the execution and coordinator are the real implementations.
//! Uses:
//! rust/crates/
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/execution.rs
//! `-- session-runtime/src/runner/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::json
//! `-- tokio::sync::watch
//! Used by:
//! `-- none, entrypoint
//! Invariants: gates are latches, no sleeps decide outcomes. Every test lets its drains settle.
//! Contract tests: session-runtime/tests/coordinator_execution.rs

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_session_api::operations::SessionExecution;
use opencode_session_api::{LocationRef, SessionError, SessionID, SessionInfo, SessionResult};
use opencode_session_runtime::execution::{LocalSessionExecution, LocationRunners, SessionLookup};
use opencode_session_runtime::runner::SessionRunner;
use serde_json::json;
use tokio::sync::watch;

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
        receiver.wait_for(|open| *open).await.expect("latch alive");
    }
}

fn id(value: &str) -> SessionID {
    serde_json::from_value(json!(value)).unwrap()
}

fn session(session_id: &str, directory: &str) -> SessionInfo {
    serde_json::from_value(json!({
        "id": session_id,
        "projectID": "global",
        "title": "test",
        "cost": 0,
        "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
        "time": { "created": 0, "updated": 0 },
        "location": { "directory": directory },
    }))
    .unwrap()
}

/// Fixture `SessionStore`: mutable map plus lookup counter.
#[derive(Default)]
struct Store {
    sessions: Mutex<HashMap<SessionID, SessionInfo>>,
    lookups: AtomicUsize,
}

impl Store {
    fn put(&self, info: SessionInfo) {
        self.sessions.lock().unwrap().insert(info.id.clone(), info);
    }
}

#[async_trait]
impl SessionLookup for Store {
    async fn get(&self, session_id: &SessionID) -> SessionResult<Option<SessionInfo>> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Ok(self.sessions.lock().unwrap().get(session_id).cloned())
    }
}

/// Fixture runner: records `(session, force)`, optionally blocks on a gate.
struct Runner {
    calls: Mutex<Vec<(SessionID, bool)>>,
    started: Latch,
    gate: Option<Latch>,
    fail: Option<String>,
}

impl Runner {
    fn new(gate: Option<Latch>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            started: Latch::new(),
            gate,
            fail: None,
        })
    }
    fn calls(&self) -> Vec<(SessionID, bool)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SessionRunner for Runner {
    async fn run(&self, session_id: &SessionID, force: bool) -> Result<(), SessionError> {
        self.calls.lock().unwrap().push((session_id.clone(), force));
        self.started.open();
        if let Some(gate) = &self.gate {
            gate.wait().await;
        }
        match &self.fail {
            Some(message) => Err(SessionError::Run {
                message: message.clone(),
                retryable: false,
            }),
            None => Ok(()),
        }
    }
}

/// Fixture `LocationServiceMap`: one runner per directory, records each resolution.
struct Locations {
    runners: HashMap<String, Arc<Runner>>,
    resolved: Mutex<Vec<String>>,
}

impl LocationRunners for Locations {
    fn get(&self, location: &LocationRef) -> Arc<dyn SessionRunner> {
        let directory = location.directory.as_str().to_owned();
        self.resolved.lock().unwrap().push(directory.clone());
        self.runners[&directory].clone()
    }
}

fn locations(runners: &[(&str, Arc<Runner>)]) -> Arc<Locations> {
    Arc::new(Locations {
        runners: runners
            .iter()
            .map(|(dir, runner)| ((*dir).to_owned(), runner.clone()))
            .collect(),
        resolved: Mutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn placement_lookup_happens_per_drain_not_at_construction() {
    let store = Arc::new(Store::default());
    let a = Runner::new(None);
    let b = Runner::new(None);
    let map = locations(&[("/a", a.clone()), ("/b", b.clone())]);
    let execution = LocalSessionExecution::new(store.clone(), map.clone());
    assert_eq!(store.lookups.load(Ordering::SeqCst), 0);
    assert!(map.resolved.lock().unwrap().is_empty());

    // The Session is created after construction and moved between drains.
    store.put(session("ses_one", "/a"));
    execution.resume(&id("ses_one")).await.unwrap();
    store.put(session("ses_one", "/b"));
    execution.resume(&id("ses_one")).await.unwrap();

    assert_eq!(store.lookups.load(Ordering::SeqCst), 2);
    assert_eq!(*map.resolved.lock().unwrap(), vec!["/a", "/b"]);
    assert_eq!(a.calls(), vec![(id("ses_one"), true)]);
    assert_eq!(b.calls(), vec![(id("ses_one"), true)]);
}

#[tokio::test]
async fn different_locations_route_to_different_runners_concurrently() {
    let store = Arc::new(Store::default());
    let gate = Latch::new();
    let a = Runner::new(Some(gate.clone()));
    let b = Runner::new(Some(gate.clone()));
    let execution = LocalSessionExecution::new(
        store.clone(),
        locations(&[("/a", a.clone()), ("/b", b.clone())]),
    );
    store.put(session("ses_a", "/a"));
    store.put(session("ses_b", "/b"));

    let first = tokio::spawn({
        let execution = execution.clone();
        async move { execution.resume(&id("ses_a")).await }
    });
    let second = tokio::spawn({
        let execution = execution.clone();
        async move { execution.resume(&id("ses_b")).await }
    });
    a.started.wait().await;
    b.started.wait().await;
    assert_eq!(
        execution.active().await.into_iter().collect::<Vec<_>>(),
        vec![id("ses_a"), id("ses_b")]
    );
    gate.open();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();

    assert_eq!(a.calls(), vec![(id("ses_a"), true)]);
    assert_eq!(b.calls(), vec![(id("ses_b"), true)]);
    assert!(execution.active().await.is_empty());
}

#[tokio::test]
async fn concurrent_resumes_join_one_drain() {
    let store = Arc::new(Store::default());
    let gate = Latch::new();
    let runner = Runner::new(Some(gate.clone()));
    let execution = LocalSessionExecution::new(store.clone(), locations(&[("/a", runner.clone())]));
    store.put(session("ses_one", "/a"));

    let first = tokio::spawn({
        let execution = execution.clone();
        async move { execution.resume(&id("ses_one")).await }
    });
    runner.started.wait().await;
    // `join!` polls in order: the second resume subscribes to the entry before the gate opens.
    let session_id = id("ses_one");
    let (second, ()) = tokio::join!(execution.resume(&session_id), async { gate.open() });
    second.unwrap();
    first.await.unwrap().unwrap();

    assert_eq!(runner.calls(), vec![(id("ses_one"), true)]);
    assert_eq!(store.lookups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn joined_resumes_share_the_failure() {
    let store = Arc::new(Store::default());
    let gate = Latch::new();
    let runner = Arc::new(Runner {
        calls: Mutex::new(Vec::new()),
        started: Latch::new(),
        gate: Some(gate.clone()),
        fail: Some("boom".to_owned()),
    });
    let execution = LocalSessionExecution::new(store.clone(), locations(&[("/a", runner.clone())]));
    store.put(session("ses_one", "/a"));

    let first = tokio::spawn({
        let execution = execution.clone();
        async move { execution.resume(&id("ses_one")).await }
    });
    runner.started.wait().await;
    let session_id = id("ses_one");
    let (second, ()) = tokio::join!(execution.resume(&session_id), async { gate.open() });
    for error in [second.unwrap_err(), first.await.unwrap().unwrap_err()] {
        assert!(matches!(&error, SessionError::Run { message, .. } if message == "boom"));
    }
    assert_eq!(runner.calls().len(), 1);
}

#[tokio::test]
async fn wakes_during_a_drain_coalesce_into_one_successor() {
    let store = Arc::new(Store::default());
    let gate = Latch::new();
    let runner = Runner::new(Some(gate.clone()));
    let execution = LocalSessionExecution::new(store.clone(), locations(&[("/a", runner.clone())]));
    store.put(session("ses_one", "/a"));

    execution.wake(&id("ses_one")).await;
    runner.started.wait().await;
    execution.wake(&id("ses_one")).await;
    execution.wake(&id("ses_one")).await;
    execution.wake(&id("ses_one")).await;
    gate.open();
    // A resume issued now joins the in-flight entry, which settles after the successor.
    execution.resume(&id("ses_one")).await.unwrap();

    assert_eq!(
        runner.calls(),
        vec![(id("ses_one"), false), (id("ses_one"), false)]
    );
    // Placement is resolved again for the coalesced successor.
    assert_eq!(store.lookups.load(Ordering::SeqCst), 2);
    assert!(execution.active().await.is_empty());
}

#[tokio::test]
async fn interrupt_is_a_noop_when_idle() {
    let store = Arc::new(Store::default());
    let runner = Runner::new(None);
    let execution = LocalSessionExecution::new(store.clone(), locations(&[("/a", runner.clone())]));
    execution.interrupt(&id("ses_one")).await;
    assert!(execution.active().await.is_empty());
    assert_eq!(store.lookups.load(Ordering::SeqCst), 0);
    assert!(runner.calls().is_empty());
}

#[tokio::test]
async fn interrupt_cancels_an_active_drain_and_resume_reports_cancelled() {
    let store = Arc::new(Store::default());
    // Gate never opens: only interruption can end the drain.
    let runner = Runner::new(Some(Latch::new()));
    let execution = LocalSessionExecution::new(store.clone(), locations(&[("/a", runner.clone())]));
    store.put(session("ses_one", "/a"));

    let pending = tokio::spawn({
        let execution = execution.clone();
        async move { execution.resume(&id("ses_one")).await }
    });
    runner.started.wait().await;
    execution.interrupt(&id("ses_one")).await;
    assert!(execution.active().await.is_empty());
    assert!(matches!(
        pending.await.unwrap(),
        Err(SessionError::Cancelled)
    ));
}

#[tokio::test]
async fn missing_session_fails_the_drain_with_not_found() {
    let store = Arc::new(Store::default());
    let runner = Runner::new(None);
    let map = locations(&[("/a", runner.clone())]);
    let execution = LocalSessionExecution::new(store.clone(), map.clone());

    let error = execution.resume(&id("ses_missing")).await.unwrap_err();
    assert!(
        matches!(&error, SessionError::NotFound { session_id } if *session_id == id("ses_missing"))
    );
    assert_eq!(error.tag(), "Session.NotFoundError");
    assert!(map.resolved.lock().unwrap().is_empty());
    assert!(runner.calls().is_empty());
    assert!(execution.active().await.is_empty());

    // An advisory wake for a missing Session settles without panicking the owner task.
    execution.wake(&id("ses_missing")).await;
    execution.resume(&id("ses_missing")).await.unwrap_err();
    assert!(execution.active().await.is_empty());
}
