//! Purpose: live launcher tests against real shell-script children that mimic `opencode serve`
//! stdout readiness, exits, hangs and grandchildren.
//! Scope: `create_opencode_server`, `create_opencode_tui`, `parse_url`, process-tree stop.
//! Uses:
//! rust/crates/
//! |-- sdk-embedded/src/launcher.rs
//! `-- sdk-embedded/src/lib.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! |-- tokio::join
//! |-- tokio::spawn
//! |-- tokio::sync::Mutex
//! |-- tokio::sync::Mutex::const_new
//! |-- tokio::test
//! |-- tokio::time::sleep
//! |-- tokio::time::timeout
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: live processes, no mocks. Unix only (scripts use /bin/sh).
//! Contract tests: self
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use opencode_sdk_embedded::launcher::parse_url;
use opencode_sdk_embedded::{
    LaunchError, ServerOptions, TuiOptions, create_opencode_server, create_opencode_tui,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

// Keep the separate shell scenarios from racing their short startup deadlines on macOS,
// where concurrent execs can stall in dyld before even `exit 0` runs. The explicit concurrent
// launch test below still exercises independent child supervisors together.
static PROCESSES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-opencode");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn options(program: &Path) -> ServerOptions {
    ServerOptions {
        program: program.into(),
        timeout: Duration::from_secs(5),
        ..ServerOptions::default()
    }
}

fn alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

async fn wait_dead(pid: i32) -> bool {
    for _ in 0..50 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[test]
fn defaults_match_server_ts() {
    let options = ServerOptions::default();
    assert_eq!(options.program, "opencode");
    assert_eq!(options.hostname, "127.0.0.1");
    assert_eq!(options.port, 4096);
    assert_eq!(options.timeout, Duration::from_millis(5000));
}

#[test]
fn parse_url_matches_regex() {
    let line = "opencode server listening on http://127.0.0.1:4096";
    assert_eq!(parse_url(line).as_deref(), Some("http://127.0.0.1:4096"));
    assert_eq!(
        parse_url("opencode server listening on  https://h:1/x extra").as_deref(),
        Some("https://h:1/x")
    );
    assert_eq!(parse_url("opencode server listening"), None);
    assert_eq!(parse_url("opencode server listening on ftp://x"), None);
    // `on` inside a word still matches like the JS regex (no word boundary).
    assert_eq!(
        parse_url("ready: con http://a").as_deref(),
        Some("http://a")
    );
}

#[tokio::test]
async fn ready_line_passes_flags_and_config_env() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let record = dir.path().join("record");
    let program = script(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > '{0}'\nprintf '%s' \"$OPENCODE_CONFIG_CONTENT\" > '{0}.env'\necho 'starting'\necho 'opencode server listening on http://127.0.0.1:5555'\nexec sleep 30",
            record.display()
        ),
    );
    let server = create_opencode_server(ServerOptions {
        hostname: "0.0.0.0".into(),
        port: 5555,
        config: Some(serde_json::json!({"logLevel": "DEBUG", "model": "a/b"})),
        ..options(&program)
    })
    .await
    .unwrap();
    assert_eq!(server.url, "http://127.0.0.1:5555");
    let args = std::fs::read_to_string(&record).unwrap();
    assert_eq!(
        args,
        "serve\n--hostname=0.0.0.0\n--port=5555\n--log-level=DEBUG\n"
    );
    let env = std::fs::read_to_string(format!("{}.env", record.display())).unwrap();
    assert_eq!(env, r#"{"logLevel":"DEBUG","model":"a/b"}"#);
    let status = server.close().await.expect("joined exit status");
    assert!(!status.success());
}

#[tokio::test]
async fn config_defaults_to_empty_object_without_log_level() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let record = dir.path().join("record");
    let program = script(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > '{0}'\nprintf '%s' \"$OPENCODE_CONFIG_CONTENT\" > '{0}.env'\necho 'opencode server listening on http://x'\nexec sleep 30",
            record.display()
        ),
    );
    let server = create_opencode_server(options(&program)).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "serve\n--hostname=127.0.0.1\n--port=4096\n"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{}.env", record.display())).unwrap(),
        "{}"
    );
    server.close().await;
}

#[tokio::test]
async fn timeout_stops_child() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let pidfile = dir.path().join("pid");
    let program = script(
        dir.path(),
        &format!("echo $$ > '{}'\nexec sleep 30", pidfile.display()),
    );
    let error = create_opencode_server(ServerOptions {
        timeout: Duration::from_millis(2000),
        ..options(&program)
    })
    .await
    .unwrap_err();
    assert!(matches!(error, LaunchError::Timeout(_)));
    assert_eq!(
        error.to_string(),
        "Timeout waiting for server to start after 2000ms"
    );
    // On a loaded machine the shell may be killed before it records its pid, give a late writer
    // time to appear. Either way nothing may be left running.
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Ok(pid) = std::fs::read_to_string(&pidfile) {
        assert!(wait_dead(pid.trim().parse().unwrap()).await);
    }
}

#[tokio::test]
async fn unparseable_ready_line_fails() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let program = script(
        dir.path(),
        "echo 'opencode server listening somewhere'\nexec sleep 30",
    );
    let error = create_opencode_server(options(&program)).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Failed to parse server url from output: opencode server listening somewhere"
    );
}

#[tokio::test]
async fn ready_prefix_on_stderr_is_ignored() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let program = script(
        dir.path(),
        "echo 'opencode server listening on http://bad' >&2\nsleep 0.2\necho 'opencode server listening on http://good'\nexec sleep 30",
    );
    let server = create_opencode_server(options(&program)).await.unwrap();
    assert_eq!(server.url, "http://good");
    server.close().await;
}

#[tokio::test]
async fn early_exit_reports_code_and_output() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let program = script(dir.path(), "echo out\necho err >&2\nexit 3");
    let error = create_opencode_server(options(&program)).await.unwrap_err();
    let LaunchError::Exited { code, output } = &error else {
        panic!("unexpected {error:?}");
    };
    assert_eq!(*code, Some(3));
    assert!(output.contains("out\n") && output.contains("err\n"));
    assert!(
        error
            .to_string()
            .starts_with("Server exited with code 3\nServer output: ")
    );
}

#[tokio::test]
async fn silent_exit_has_no_output_suffix() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let program = script(dir.path(), "exit 0");
    let error = create_opencode_server(options(&program)).await.unwrap_err();
    assert_eq!(error.to_string(), "Server exited with code 0");
}

#[tokio::test]
async fn missing_program_is_spawn_error() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let error = create_opencode_server(options(&dir.path().join("nope")))
        .await
        .unwrap_err();
    assert!(matches!(error, LaunchError::Spawn(_)));
}

#[tokio::test]
async fn abort_before_spawn_and_during_startup() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let pidfile = dir.path().join("pid");
    let program = script(
        dir.path(),
        &format!("echo $$ > '{}'\nexec sleep 30", pidfile.display()),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = create_opencode_server(ServerOptions {
        cancel: Some(cancel),
        ..options(&program)
    })
    .await
    .unwrap_err();
    assert!(matches!(error, LaunchError::Aborted));
    assert!(!pidfile.exists(), "aborted signal must not spawn");

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let watched = pidfile.clone();
    tokio::spawn(async move {
        // Abort once the child is really running, independent of machine load.
        while !watched.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        trigger.cancel();
    });
    let error = create_opencode_server(ServerOptions {
        cancel: Some(cancel),
        ..options(&program)
    })
    .await
    .unwrap_err();
    assert!(matches!(error, LaunchError::Aborted));
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(wait_dead(pid).await);
}

#[tokio::test]
async fn abort_after_ready_and_close_kill_process_tree() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let grandchild = dir.path().join("grandchild");
    let program = script(
        dir.path(),
        &format!(
            "sleep 30 &\necho $! > '{}'\necho 'opencode server listening on http://127.0.0.1:1'\nwhile true; do echo tick; sleep 0.05; done",
            grandchild.display()
        ),
    );
    let cancel = CancellationToken::new();
    let server = create_opencode_server(ServerOptions {
        cancel: Some(cancel.clone()),
        ..options(&program)
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(&grandchild)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    // Output keeps draining after readiness, the child is not blocked on a full pipe.
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();
    assert!(wait_dead(pid).await, "grandchild survived abort");
    let status = server.close().await.expect("joined exit status");
    assert!(!status.success());
}

#[tokio::test]
async fn drop_stops_server() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let pidfile = dir.path().join("pid");
    let program = script(
        dir.path(),
        &format!(
            "echo $$ > '{}'\necho 'opencode server listening on http://x'\nexec sleep 30",
            pidfile.display()
        ),
    );
    let server = create_opencode_server(options(&program)).await.unwrap();
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    drop(server);
    assert!(wait_dead(pid).await);
}

#[tokio::test]
async fn tui_passes_flags_env_and_closes() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let record = dir.path().join("record");
    let program = script(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > '{0}'\nprintf '%s' \"$OPENCODE_CONFIG_CONTENT\" > '{0}.env'\nexec sleep 30",
            record.display()
        ),
    );
    let tui = create_opencode_tui(TuiOptions {
        program: program.into(),
        project: Some("/p".into()),
        model: Some("a/b".into()),
        session: Some(String::new()),
        agent: Some("build".into()),
        config: Some(serde_json::json!({"x": 1})),
        cancel: None,
    })
    .unwrap();
    // TUI launch returns immediately and promises no readiness deadline. Wait for the actual
    // child write, using the existing server startup watchdog only to bound this fixture.
    tokio::time::timeout(ServerOptions::default().timeout, async {
        loop {
            if std::fs::read_to_string(format!("{}.env", record.display()))
                .ok()
                .as_deref()
                == Some(r#"{"x":1}"#)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("TUI fixture must write its complete configuration record");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "--project=/p\n--model=a/b\n--agent=build\n"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{}.env", record.display())).unwrap(),
        r#"{"x":1}"#
    );
    let status = tui.close().await.expect("joined exit status");
    assert!(!status.success());
}

#[tokio::test]
async fn tui_abort_and_natural_exit() {
    let _processes = PROCESSES.lock().await;
    let dir = TempDir::new().unwrap();
    let program = script(dir.path(), "exit 4");
    let tui = create_opencode_tui(TuiOptions {
        program: program.clone().into(),
        ..TuiOptions::default()
    })
    .unwrap();
    assert_eq!(tui.wait().await.and_then(|s| s.code()), Some(4));

    let program = script(dir.path(), "exec sleep 30");
    let cancel = CancellationToken::new();
    let tui = create_opencode_tui(TuiOptions {
        program: program.into(),
        cancel: Some(cancel.clone()),
        ..TuiOptions::default()
    })
    .unwrap();
    cancel.cancel();
    let status = tokio::time::timeout(Duration::from_secs(5), tui.wait())
        .await
        .expect("abort stops tui")
        .expect("status");
    assert!(!status.success());
}

#[tokio::test]
async fn concurrent_servers_keep_readiness_and_shutdown_independent() {
    let _processes = PROCESSES.lock().await;
    let left_root = TempDir::new().unwrap();
    let right_root = TempDir::new().unwrap();
    let left_program = script(
        left_root.path(),
        "echo 'opencode server listening on http://left'\nexec sleep 30",
    );
    let right_program = script(
        right_root.path(),
        "echo 'opencode server listening on http://right'\nexec sleep 30",
    );
    let (left, right) = tokio::join!(
        create_opencode_server(options(&left_program)),
        create_opencode_server(options(&right_program))
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.url, "http://left");
    assert_eq!(right.url, "http://right");
    let (left, right) = tokio::join!(left.close(), right.close());
    assert!(!left.unwrap().success());
    assert!(!right.unwrap().success());
}
