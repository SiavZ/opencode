//! Purpose: shared fixtures for the WSL controller tests: a scripted fake `WslRunner` with gated
//! command kinds, fake sidecar handles, spawners, an in-memory store and a polling helper.
//! Scope: test fixture module (not a delivery; the real adapter is `platform/wsl/exe.rs`).
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/platform/process_tree.rs
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/runtime.rs
//! `-- desktop-host/src/platform/wsl/sidecar.rs
//! External:
//! |-- futures::future::BoxFuture
//! `-- tokio::sync::Semaphore
//! Used by:
//! rust/crates/
//! `-- desktop-host/tests/wsl_servers.rs
//! Invariants: gated commands resolve `Aborted` when their cancel token fires.
//! Contract tests: desktop-host/tests/wsl_servers.rs
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use opencode_desktop_host::connections::ServerReadyData;
use opencode_desktop_host::platform::process_tree::ExitInfo;
use opencode_desktop_host::platform::wsl::WslServerConfig;
use opencode_desktop_host::platform::wsl::controller::{
    MemoryStore, RunningSidecar, SidecarHandle, SpawnSidecar, WslServersController,
};
use opencode_desktop_host::platform::wsl::runtime::{
    RunOptions, WslCommandResult, WslError, WslRunner,
};
use parking_lot::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

// ---------- fake runner (fixture) ----------

/// Scripted runner. Commands are classified by their shell payload; gated kinds block until the
/// test releases them, which lets tests interleave removal/cancellation with in-flight work.
#[derive(Default)]
pub struct FakeRunner {
    pub calls: Mutex<Vec<(String, String)>>,
    pub gates: Mutex<HashMap<String, Arc<Semaphore>>>,
    pub gated: Mutex<Vec<String>>,
    pub cannot_execute: Mutex<Vec<String>>,
    pub version: Mutex<String>,
}

impl FakeRunner {
    pub fn new(version: &str) -> Arc<Self> {
        let runner = Self::default();
        *runner.version.lock() = version.to_string();
        Arc::new(runner)
    }
    pub fn gate(&self, kind: &str) {
        self.gated.lock().push(kind.to_string());
    }
    pub fn release(&self, key: &str) {
        self.semaphore(key).add_permits(1000);
    }
    fn semaphore(&self, key: &str) -> Arc<Semaphore> {
        self.gates
            .lock()
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(0)))
            .clone()
    }
    pub fn calls(&self, kind: &str) -> Vec<String> {
        self.calls
            .lock()
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, d)| d.clone())
            .collect()
    }
}

pub fn classify(args: &[String]) -> (String, String) {
    let distro = args
        .iter()
        .position(|a| a == "-d")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let joined = args.join(" ");
    let kind = if args.iter().any(|a| a == "/bin/true") {
        "probe"
    } else if joined.contains("command -v bash") {
        "bash"
    } else if joined.contains("command -v curl") {
        "curl"
    } else if joined.contains("--version 2>/dev/null") {
        "version"
    } else if joined.contains(".opencode/bin/opencode") {
        "resolve"
    } else if args.first().is_some_and(|a| a == "--version") {
        "runtime"
    } else if joined.contains("--list --verbose") {
        "installed"
    } else if joined.contains("--list --online") {
        "online"
    } else {
        "other"
    };
    (kind.to_string(), distro)
}

pub fn ok(stdout: &str) -> WslCommandResult {
    WslCommandResult {
        code: Some(0),
        signal: None,
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

/// Newtype so the foreign `WslRunner` trait can be implemented for the shared fake.
pub struct Runner(pub Arc<FakeRunner>);

impl WslRunner for Runner {
    fn run(
        &self,
        _program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>> {
        let this = self.0.clone();
        async move {
            let (kind, distro) = classify(&args);
            this.calls.lock().push((kind.clone(), distro.clone()));
            if this.gated.lock().contains(&kind) {
                let gate = this.semaphore(&format!("{kind}:{distro}"));
                let cancel = options.cancel.clone().unwrap_or_default();
                tokio::select! {
                    permit = gate.acquire() => drop(permit),
                    () = cancel.cancelled() => return Err(WslError::Aborted),
                }
            }
            Ok(match kind.as_str() {
                "probe" if this.cannot_execute.lock().contains(&distro) => WslCommandResult {
                    code: Some(1),
                    signal: None,
                    stdout: String::new(),
                    stderr: format!("Open {distro} once to finish setup"),
                },
                "probe" => ok(""),
                "bash" | "curl" => ok("yes"),
                "resolve" => ok("/home/me/.opencode/bin/opencode\n"),
                "version" => ok(&this.version.lock()),
                "runtime" => ok("WSL version: 2.6.1\nKernel version: 6"),
                "installed" => {
                    ok("  NAME      STATE           VERSION\n* Debian    Running         2\n")
                }
                "online" => ok("NAME            FRIENDLY NAME\nUbuntu-24.04    Ubuntu 24.04 LTS\n"),
                _ => ok(""),
            })
        }
        .boxed()
    }

    fn run_interactive(
        &self,
        program: &str,
        args: Vec<String>,
        options: RunOptions,
    ) -> BoxFuture<'static, Result<WslCommandResult, WslError>> {
        self.run(program, args, options)
    }

    fn spawn_detached(
        &self,
        program: &str,
        args: Vec<String>,
    ) -> BoxFuture<'static, Result<(), WslError>> {
        self.0
            .calls
            .lock()
            .push((program.to_string(), args.join(" ")));
        async { Ok(()) }.boxed()
    }
}

// ---------- fake sidecar handles ----------

pub struct FakeHandle {
    pub stopped: AtomicBool,
    pub exit: tokio::sync::watch::Sender<Option<ExitInfo>>,
}

impl FakeHandle {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            stopped: AtomicBool::new(false),
            exit: tokio::sync::watch::channel(None).0,
        })
    }
    pub fn exit_with(&self, code: i32) {
        let _ = self.exit.send(Some(ExitInfo {
            code: Some(code),
            signal: None,
        }));
    }
}

impl SidecarHandle for FakeHandle {
    fn stop(&self) -> BoxFuture<'_, ()> {
        self.stopped.store(true, Ordering::SeqCst);
        self.exit_with(0);
        async {}.boxed()
    }
    fn exited(&self) -> BoxFuture<'_, ExitInfo> {
        let mut rx = self.exit.subscribe();
        async move {
            loop {
                if let Some(exit) = rx.borrow_and_update().clone() {
                    return exit;
                }
                if rx.changed().await.is_err() {
                    return ExitInfo {
                        code: None,
                        signal: None,
                    };
                }
            }
        }
        .boxed()
    }
}

pub fn ready_sidecar(handle: Arc<FakeHandle>, port: u16) -> RunningSidecar {
    RunningSidecar {
        handle,
        ready: ServerReadyData {
            url: format!("http://127.0.0.1:{port}"),
            username: Some("opencode".into()),
            password: Some("secret".into()),
        },
    }
}

/// Spawner that resolves immediately with a fresh handle and records every handle it produced.
pub fn instant_spawn(handles: Arc<Mutex<Vec<Arc<FakeHandle>>>>) -> SpawnSidecar {
    Arc::new(move |_distro, _cancel| {
        let handle = FakeHandle::new();
        handles.lock().push(handle.clone());
        let port = 4096 + handles.lock().len() as u16;
        async move { Ok(ready_sidecar(handle, port)) }.boxed()
    })
}

/// Spawner that never resolves until cancelled (`new Promise<never>(() => undefined)`).
pub fn pending_spawn() -> SpawnSidecar {
    Arc::new(|_distro, cancel: CancellationToken| {
        async move {
            cancel.cancelled().await;
            Err("Aborted".to_string())
        }
        .boxed()
    })
}

pub fn store(servers: &[(&str, &str)]) -> Arc<MemoryStore> {
    Arc::new(MemoryStore(Mutex::new(
        servers
            .iter()
            .map(|(id, distro)| WslServerConfig {
                id: id.to_string(),
                distro: distro.to_string(),
            })
            .collect(),
    )))
}

pub async fn wait_for(check: impl Fn() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("Timed out waiting for condition");
}

pub fn controller(
    runner: &Arc<FakeRunner>,
    spawn: SpawnSidecar,
    store: Arc<MemoryStore>,
) -> WslServersController {
    WslServersController::new("1.16.2", Arc::new(Runner(runner.clone())), spawn, store)
}
