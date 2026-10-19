//! Purpose: LIVE process tests for the owned sidecar: spawn a real HTTP server child
//! (`python3 -m http.server`), prove health over HTTP, stop it through `SidecarSlot::kill_sidecar`,
//! and prove no process of its group survives; plus startup failure propagation and timeouts.
//! Scope: unix live tests (macOS verified); skipped with a message when `python3` is missing.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! |-- parking_lot::Mutex::new
//! |-- reqwest::Client::builder
//! |-- reqwest::Client::new
//! |-- tempfile::tempdir
//! |-- tokio::process::Command::new
//! |-- tokio::test
//! |-- tokio::time::sleep
//! |-- tokio_util::sync::CancellationToken
//! `-- which::which
//! Used by:
//! `-- none, entrypoint
//! Invariants: every spawned child is stopped and reaped before the test returns; cwd and
//! HOME/XDG point at temp dirs.
//! Contract tests: desktop-host/tests/connections_live.rs
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use opencode_desktop_host::connections::sidecar::{
    SidecarConfig, allocate_port, spawn_local_sidecar,
};
use opencode_desktop_host::connections::{
    ChildServer, ChildSpec, ConnectionError, HealthFailure, ServerConnection, SidecarSlot,
    check_health,
};
use opencode_desktop_host::platform::proxy::{EnvMap, process_env};
use tokio_util::sync::CancellationToken;

fn python() -> Option<PathBuf> {
    which::which("python3").ok()
}

fn temp_env(dir: &std::path::Path) -> EnvMap {
    let mut env = EnvMap::new();
    env.insert(
        "PATH".into(),
        process_env().get("PATH").cloned().unwrap_or_default(),
    );
    env.insert("HOME".into(), dir.to_string_lossy().to_string());
    env.insert(
        "XDG_DATA_HOME".into(),
        dir.join("data").to_string_lossy().to_string(),
    );
    env
}

/// `kill -0 -<pgid>` succeeds while any process of the group is alive.
async fn group_alive(pgid: u32) -> bool {
    tokio::process::Command::new("kill")
        .args(["-0", "--", &format!("-{pgid}")])
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

fn config(program: PathBuf, args: Vec<String>, port: u16, dir: &std::path::Path) -> SidecarConfig {
    SidecarConfig {
        program,
        args,
        hostname: "127.0.0.1".into(),
        port,
        password: "pw".into(),
        user_data_path: dir.to_path_buf(),
        base_env: temp_env(dir),
        cwd: Some(dir.to_path_buf()),
        ready_timeout: Duration::from_secs(20),
        stop_grace: Duration::from_secs(3),
        on_line: None,
    }
}

#[tokio::test]
async fn live_python_sidecar_is_healthy_then_kill_sidecar_leaves_no_survivor() {
    let Some(python) = python() else {
        eprintln!("SKIP: python3 not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    // http.server answers /global/health with 200 when the file exists.
    std::fs::create_dir_all(dir.path().join("global")).unwrap();
    std::fs::write(dir.path().join("global/health"), "{\"healthy\":true}").unwrap();
    let port = allocate_port("127.0.0.1").await.unwrap();
    // `sh -c` parent + python grandchild: the stop must take the whole tree, not just the leader.
    let script = format!(
        "{} -m http.server {port} --bind 127.0.0.1 & wait",
        python.display()
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let connection = spawn_local_sidecar(
        config(
            "/bin/sh".into(),
            vec!["-c".into(), script],
            port,
            dir.path(),
        ),
        &client,
        &CancellationToken::new(),
    )
    .await
    .expect("sidecar becomes healthy");
    assert!(connection.owned_by_host());
    assert_eq!(connection.key(), "sidecar");
    let ServerConnection::OwnedSidecar(child, ready) = &connection else {
        panic!("owned sidecar expected");
    };
    assert_eq!(ready.url, format!("http://127.0.0.1:{port}"));
    assert_eq!(ready.password.as_deref(), Some("pw"));
    let pgid = child.pid().expect("pid");
    assert!(group_alive(pgid).await);
    assert!(check_health(&client, &ready.url, connection.basic_auth().as_ref()).await);

    let slot = SidecarSlot::new();
    slot.set(connection.clone()).await;
    let exit = slot.kill_sidecar().await.expect("owned sidecar is stopped");
    assert!(exit.code.is_some() || exit.signal.is_some());
    assert!(slot.current().await.is_none());
    assert!(
        !group_alive(pgid).await,
        "process group {pgid} survived stop"
    );
    assert!(!check_health(&client, &ready.url, None).await);
    // Stop is idempotent and joined.
    let ServerConnection::OwnedSidecar(child, _) = &connection else {
        unreachable!()
    };
    assert!(child.stop().await.is_some());
}

#[tokio::test]
async fn live_child_exit_before_healthy_propagates_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let port = allocate_port("127.0.0.1").await.unwrap();
    let client = reqwest::Client::new();
    let error = spawn_local_sidecar(
        config(
            "/bin/sh".into(),
            vec!["-c".into(), "echo boom >&2; exit 3".into()],
            port,
            dir.path(),
        ),
        &client,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    let ConnectionError::ExitedBeforeHealthy(exit) = &error else {
        panic!("exit error expected, got {error:?}");
    };
    assert_eq!(exit.code, Some(3));
    assert_eq!(
        error.to_string(),
        "Sidecar exited before health check passed with code 3"
    );
}

#[tokio::test]
async fn live_ready_timeout_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let port = allocate_port("127.0.0.1").await.unwrap();
    let client = reqwest::Client::new();
    let mut cfg = config(
        "/bin/sh".into(),
        vec!["-c".into(), "sleep 30".into()],
        port,
        dir.path(),
    );
    cfg.ready_timeout = Duration::from_millis(300);
    let error = spawn_local_sidecar(cfg, &client, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ConnectionError::ReadyTimeout {
            timeout_ms: 300,
            ..
        }
    ));
}

#[tokio::test]
async fn live_spawn_failure_and_cancellation_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let port = allocate_port("127.0.0.1").await.unwrap();
    let client = reqwest::Client::new();
    let missing = spawn_local_sidecar(
        config(dir.path().join("missing-binary"), vec![], port, dir.path()),
        &client,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, ConnectionError::Spawn { .. }));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let cancelled = spawn_local_sidecar(
        config(
            "/bin/sh".into(),
            vec!["-c".into(), "sleep 30".into()],
            port,
            dir.path(),
        ),
        &client,
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(matches!(cancelled, ConnectionError::Cancelled));
}

#[tokio::test]
async fn live_child_records_recent_output_and_sigterm_resistant_tree_is_force_killed() {
    let dir = tempfile::tempdir().unwrap();
    let lines = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = lines.clone();
    // Ignores SIGTERM, so stop must escalate to SIGKILL after the grace period.
    let child = ChildServer::spawn(ChildSpec {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "trap '' TERM; echo ready; echo warn >&2; sleep 30 & wait".into(),
        ],
        env: temp_env(dir.path()),
        cwd: Some(dir.path().to_path_buf()),
        stdin: None,
        url: "http://127.0.0.1:9".into(),
        auth: None,
        stop_grace: Duration::from_millis(300),
        on_line: Some(Arc::new(move |line| sink.lock().push(line.text))),
    })
    .await
    .unwrap();
    let pgid = child.pid().unwrap();
    for _ in 0..50 {
        if child.recent_output().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut recent = child.recent_output();
    recent.sort();
    assert_eq!(recent, ["[stderr] warn", "[stdout] ready"]);
    let started = std::time::Instant::now();
    let exit = child.stop().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(exit.signal.as_deref(), Some("SIGKILL"));
    assert!(!group_alive(pgid).await);
    let mut seen = lines.lock().clone();
    seen.sort();
    assert_eq!(seen, ["ready", "warn"]);
}

#[tokio::test]
async fn live_wait_healthy_reports_exit_with_recent_output() {
    let dir = tempfile::tempdir().unwrap();
    let child = ChildServer::spawn(ChildSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo failing >&2; sleep 0.2; exit 7".into()],
        env: temp_env(dir.path()),
        cwd: None,
        stdin: None,
        url: "http://127.0.0.1:9".into(),
        auth: None,
        stop_grace: Duration::ZERO,
        on_line: None,
    })
    .await
    .unwrap();
    let failure = child
        .wait_healthy(
            &reqwest::Client::new(),
            Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let HealthFailure::Exited { exit, recent } = failure else {
        panic!("exit expected");
    };
    assert_eq!(exit.code, Some(7));
    assert_eq!(recent, ["[stderr] failing"]);
}

#[tokio::test]
async fn live_stdin_script_is_delivered_like_wsl_bash_se() {
    let child = ChildServer::spawn(ChildSpec {
        program: "/bin/sh".into(),
        args: vec!["-s".into()],
        env: EnvMap::new(),
        cwd: None,
        stdin: Some("echo from-stdin\nexit 0\n".into()),
        url: "http://127.0.0.1:9".into(),
        auth: None,
        stop_grace: Duration::ZERO,
        on_line: None,
    })
    .await
    .unwrap();
    let exit = child.exited().await;
    assert_eq!(exit.code, Some(0));
    assert_eq!(child.recent_output(), ["[stdout] from-stdin"]);
}
