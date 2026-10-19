//! Purpose: port of the controller half of `packages/desktop/src/main/wsl/servers.test.ts` against
//! `WslServersController` with a scripted fake `WslRunner` (fixture), plus generation-counter,
//! job-cancellation, exit-watch and stop-all checks. Pure policy tests live in `wsl_policy.rs`.
//! Scope: fixture tests (no wsl.exe; fixtures in `wsl_support/mod.rs`). The real adapter is
//! covered by `wsl_runtime.rs`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! `-- desktop-host/tests/wsl_support/mod.rs
//! External:
//! |-- futures::FutureExt
//! |-- parking_lot::Mutex
//! |-- tokio::spawn
//! |-- tokio::sync::Semaphore
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: every controller is shut down (tasks joined) before the test ends.
//! Contract tests: desktop-host/tests/wsl_servers.rs

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::FutureExt;
use opencode_desktop_host::platform::wsl::controller::SpawnSidecar;
use opencode_desktop_host::platform::wsl::{WslJob, WslServerRuntime};
use parking_lot::Mutex;
use tokio::sync::Semaphore;

mod wsl_support;

use wsl_support::{
    FakeHandle, FakeRunner, controller, instant_spawn, pending_spawn, ready_sidecar, store,
    wait_for,
};

// ---------- controller (servers.test.ts:101-217) ----------

#[tokio::test]
async fn ignores_stale_background_opencode_checks_after_removing_a_wsl_server() {
    let runner = FakeRunner::new("1.16.2");
    runner.gate("resolve");
    let handles = Arc::new(Mutex::new(Vec::new()));
    let persisted = store(&[]);
    let c = controller(&runner, instant_spawn(handles.clone()), persisted.clone());
    c.add_server("Debian").unwrap();
    assert_eq!(persisted.0.lock().len(), 1);
    wait_for(|| !runner.calls("resolve").is_empty()).await;
    c.remove_server("wsl:Debian").await;
    runner.release("resolve:Debian");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(c.state().servers.is_empty());
    assert!(c.state().opencode_checks.is_empty());
    assert!(persisted.0.lock().is_empty());
    // The removed server's sidecar was stopped.
    assert!(handles.lock()[0].stopped.load(Ordering::SeqCst));
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn ignores_stale_startup_opencode_checks_after_removing_a_wsl_server() {
    let runner = FakeRunner::new("1.16.2");
    runner.gate("resolve");
    let c = controller(&runner, pending_spawn(), store(&[("wsl:Debian", "Debian")]));
    c.initialize();
    wait_for(|| !runner.calls("resolve").is_empty()).await;
    assert_eq!(c.state().servers[0].runtime, WslServerRuntime::Starting);
    c.remove_server("wsl:Debian").await;
    runner.release("resolve:Debian");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(c.state().servers.is_empty());
    assert!(c.state().opencode_checks.is_empty());
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn probes_addable_distros_in_parallel_before_checking_opencode() {
    let runner = FakeRunner::new("1.16.2");
    runner.gate("probe");
    let c = controller(&runner, pending_spawn(), store(&[]));
    let task = {
        let c = c.clone();
        tokio::spawn(async move { c.probe_addable(&["Debian".into(), "Ubuntu".into()]).await })
    };
    wait_for(|| runner.calls("probe").len() == 2).await;
    assert_eq!(runner.calls("probe"), ["Debian", "Ubuntu"]);
    assert!(runner.calls("resolve").is_empty());
    assert!(matches!(c.state().job, Some(WslJob::ProbeAddable { .. })));
    runner.release("probe:Debian");
    runner.release("probe:Ubuntu");
    task.await.unwrap().unwrap();
    let state = c.state();
    assert_eq!(state.distro_probes.keys(), ["Debian", "Ubuntu"]);
    let mut resolved = runner.calls("resolve");
    resolved.sort();
    assert_eq!(resolved, ["Debian", "Ubuntu"]);
    assert_eq!(state.opencode_checks.keys(), ["Debian", "Ubuntu"]);
    assert_eq!(
        state.opencode_checks.get("Debian").unwrap().matches_desktop,
        Some(true)
    );
    assert_eq!(state.job, None);
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn does_not_check_opencode_in_addable_distros_that_cannot_execute_commands() {
    let runner = FakeRunner::new("1.16.2");
    runner.cannot_execute.lock().push("Ubuntu".into());
    let c = controller(&runner, pending_spawn(), store(&[]));
    c.probe_addable(&["Debian".into(), "Ubuntu".into()])
        .await
        .unwrap();
    let state = c.state();
    assert_eq!(state.distro_probes.keys(), ["Debian", "Ubuntu"]);
    assert_eq!(
        state.distro_probes.get("Ubuntu").unwrap().error.as_deref(),
        Some("Open Ubuntu once to finish setup")
    );
    assert_eq!(runner.calls("resolve"), ["Debian"]);
    assert_eq!(state.opencode_checks.keys(), ["Debian"]);
    c.shutdown(Duration::from_secs(1)).await;
}

// ---------- generation counters, jobs, exits, quit ----------

#[tokio::test]
async fn stale_start_generation_is_disposed_and_never_published() {
    let runner = FakeRunner::new("1.16.2");
    let first_gate = Arc::new(Semaphore::new(0));
    let handles: Arc<Mutex<Vec<Arc<FakeHandle>>>> = Arc::new(Mutex::new(Vec::new()));
    let spawn: SpawnSidecar = {
        let handles = handles.clone();
        let gate = first_gate.clone();
        Arc::new(move |_distro, _cancel| {
            let handle = FakeHandle::new();
            let index = {
                let mut list = handles.lock();
                list.push(handle.clone());
                list.len()
            };
            let gate = gate.clone();
            async move {
                if index == 1 {
                    drop(gate.acquire().await);
                }
                Ok(ready_sidecar(handle, 5000 + index as u16))
            }
            .boxed()
        })
    };
    let c = controller(&runner, spawn, store(&[("wsl:Debian", "Debian")]));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    c.subscribe(move |event| sink.lock().push(event.clone()));
    c.initialize();
    wait_for(|| handles.lock().len() == 1).await;
    // A second start (e.g. restart after install) supersedes the slow first one.
    c.start_server("wsl:Debian").await;
    let WslServerRuntime::Ready { url, .. } = c.state().servers[0].runtime.clone() else {
        panic!("second start should be ready");
    };
    assert_eq!(url, "http://127.0.0.1:5002");
    first_gate.add_permits(1);
    wait_for(|| handles.lock()[0].stopped.load(Ordering::SeqCst)).await;
    let WslServerRuntime::Ready { url, .. } = c.state().servers[0].runtime.clone() else {
        panic!("stale start must not overwrite the current runtime");
    };
    assert_eq!(url, "http://127.0.0.1:5002");
    assert!(!handles.lock()[1].stopped.load(Ordering::SeqCst));
    assert!(!events.lock().is_empty());
    // Quit stops only what the controller owns, and joins.
    c.shutdown(Duration::from_secs(1)).await;
    assert!(handles.lock()[1].stopped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn sidecar_exit_after_startup_marks_the_server_failed() {
    let runner = FakeRunner::new("1.16.2");
    let handles = Arc::new(Mutex::new(Vec::new()));
    let c = controller(&runner, instant_spawn(handles.clone()), store(&[]));
    c.add_server("Debian").unwrap();
    assert_eq!(
        c.add_server("Debian").unwrap_err().to_string(),
        "Debian is already added"
    );
    wait_for(|| matches!(c.state().servers[0].runtime, WslServerRuntime::Ready { .. })).await;
    handles.lock()[0].exit_with(9);
    wait_for(|| {
        matches!(
            c.state().servers[0].runtime,
            WslServerRuntime::Failed { .. }
        )
    })
    .await;
    assert_eq!(
        c.state().servers[0].runtime,
        WslServerRuntime::Failed {
            message: "WSL server exited after startup (code=9 signal=null)".into()
        }
    );
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn failed_start_publishes_message_and_new_job_cancels_previous() {
    let runner = FakeRunner::new("1.16.2");
    runner.gate("runtime");
    let spawn: SpawnSidecar = Arc::new(|distro, _| {
        async move { Err(format!("OpenCode is not installed in {distro}")) }.boxed()
    });
    let c = controller(&runner, spawn, store(&[("wsl:Debian", "Debian")]));
    c.initialize();
    wait_for(|| {
        matches!(
            c.state().servers[0].runtime,
            WslServerRuntime::Failed { .. }
        )
    })
    .await;
    assert_eq!(
        c.state().servers[0].runtime,
        WslServerRuntime::Failed {
            message: "OpenCode is not installed in Debian".into()
        }
    );
    let first = {
        let c = c.clone();
        tokio::spawn(async move { c.probe_runtime().await })
    };
    wait_for(|| !runner.calls("runtime").is_empty()).await;
    assert!(matches!(c.state().job, Some(WslJob::Runtime { .. })));
    // A new job cancels the gated one; the cancelled job resolves silently.
    c.refresh_distros().await.unwrap();
    first.await.unwrap().unwrap();
    let state = c.state();
    assert_eq!(state.job, None);
    assert_eq!(state.installed[0].name, "Debian");
    assert!(state.installed[0].is_default);
    assert_eq!(state.online[0].name, "Ubuntu-24.04");
    // Baseline `probeWslRuntime` catches the abort and still publishes `available: false`
    // (runtime.ts:210-224); the job itself resolves silently.
    let runtime = state.runtime.as_ref().unwrap();
    assert!(!runtime.available);
    assert_eq!(runtime.error.as_deref(), Some("Aborted"));
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn install_opencode_verifies_version_and_restarts_server() {
    let runner = FakeRunner::new("1.14.35");
    let handles = Arc::new(Mutex::new(Vec::new()));
    let c = controller(
        &runner,
        instant_spawn(handles.clone()),
        store(&[("wsl:Debian", "Debian")]),
    );
    c.initialize();
    wait_for(|| handles.lock().len() == 1).await;
    let error = c.install_opencode("Debian").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "OpenCode update finished but Debian still reports 1.14.35; expected 1.16.2"
    );
    assert_eq!(handles.lock().len(), 1);
    *runner.version.lock() = "1.16.2".into();
    c.install_opencode("Debian").await.unwrap();
    // Restarted: the old sidecar was stopped and a new one spawned.
    assert_eq!(handles.lock().len(), 2);
    assert!(handles.lock()[0].stopped.load(Ordering::SeqCst));
    c.open_terminal("Ubuntu Preview").await.unwrap();
    assert!(
        runner
            .calls
            .lock()
            .iter()
            .any(|(p, a)| p == "cmd.exe" && a == "/c start  wsl -d Ubuntu Preview")
    );
    c.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn shutdown_cancels_pending_spawns_and_joins_tasks() {
    let runner = FakeRunner::new("1.16.2");
    let c = controller(
        &runner,
        pending_spawn(),
        store(&[("wsl:Debian", "Debian"), ("wsl:Ubuntu", "Ubuntu")]),
    );
    c.initialize();
    wait_for(|| {
        c.state()
            .servers
            .iter()
            .all(|s| s.runtime == WslServerRuntime::Starting)
    })
    .await;
    tokio::time::timeout(Duration::from_secs(2), c.shutdown(Duration::from_secs(1)))
        .await
        .expect("shutdown joins promptly");
    // Cancelled starts are stale (generation bumped by stop_all) and never publish `failed`.
    assert!(
        c.state()
            .servers
            .iter()
            .all(|s| s.runtime == WslServerRuntime::Starting)
    );
}
