//! Purpose: live conformance of the native process runner: exit codes, bounded capture, combined
//! output, live sink, timeout and cancellation with descendant termination, line streaming.
//! Scope: real /bin/sh processes in temp dirs (unix only).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! `-- platform-native/src/process/mod.rs
//! External:
//! |-- futures::StreamExt
//! |-- tempfile::tempdir
//! |-- tokio::join
//! |-- tokio::process::Command::new
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc::channel
//! |-- tokio::test
//! |-- tokio::time::sleep
//! |-- tokio::time::timeout
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: no test leaves a live descendant behind.
//! Contract tests: this integration suite
#![cfg(unix)]

use futures::StreamExt;
use opencode_platform_api::process::{
    Command, ProcessError, ProcessRunner, RunOptions, StreamOptions,
};
use opencode_platform_native::process::NativeProcessRunner;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn sh(script: &str) -> Command {
    Command::new("/bin/sh").args(["-c", script])
}

fn alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn wait_gone(pid: i32) -> bool {
    for _ in 0..100 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn stdin_after_parent_success_stays_inside_timeout_and_cancellation() {
    for cancel_run in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let pidfile = temp.path().join("holder");
        // Explicitly inherit stdin on a descendant, but close its output pipes so the leader
        // and output readers finish before the blocked 4 MiB stdin write.
        let command = sh(&format!(
            "/bin/sh -c 'sleep 2' <&0 >/dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ));
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let started = Instant::now();
        let run = NativeProcessRunner.run(
            &command,
            RunOptions {
                stdin: Some(vec![b'x'; 4 * 1024 * 1024]),
                timeout: (!cancel_run).then_some(Duration::from_millis(150)),
                cancel: Some(cancel),
                kill_grace: Some(Duration::from_millis(30)),
                ..RunOptions::default()
            },
        );
        let trigger = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if cancel_run {
                trigger.cancel();
            }
        };
        let (result, ()) = tokio::join!(run, trigger);
        let elapsed = started.elapsed();
        let pid = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = wait_gone(pid).await;
        assert!(gone, "owned stdin holder survived cleanup");
        assert!(
            elapsed < Duration::from_secs(1),
            "stdin escaped deadline: {elapsed:?}"
        );
        if cancel_run {
            assert!(
                matches!(result, Err(ProcessError::Cancelled { .. })),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(ProcessError::TimedOut { .. })),
                "{result:?}"
            );
        }
    }
}

#[tokio::test]
async fn cancelled_or_dropped_run_closes_stdin_without_killing_unrelated_processes() {
    for drop_run in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut unrelated = tokio::process::Command::new("/bin/sleep")
            .arg("10")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let command = sh(
            "(/bin/sleep 0.3; cat > drained) <&0 >/dev/null 2>&1 & echo $! > holder; /bin/sleep 1",
        )
        .cwd(temp.path());
        let cancel = CancellationToken::new();
        let run = NativeProcessRunner.run(
            &command,
            RunOptions {
                stdin: Some(vec![b'x'; 4 * 1024 * 1024]),
                // Only the leader belongs to this run's kill target. The finite descendant
                // drains the inherited pipe later, proving that our writer closed on teardown.
                detached: Some(false),
                cancel: Some(cancel.clone()),
                kill_grace: Some(Duration::from_millis(30)),
                ..RunOptions::default()
            },
        );
        let result = if drop_run {
            tokio::time::timeout(Duration::from_millis(150), run)
                .await
                .ok()
        } else {
            let trigger = async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancel.cancel();
            };
            Some(tokio::join!(run, trigger).0)
        };
        let pid = std::fs::read_to_string(temp.path().join("holder"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = wait_gone(pid).await;
        let drained = std::fs::metadata(temp.path().join("drained")).map(|info| info.len());
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        unrelated.kill().await.unwrap();
        let _ = unrelated.wait().await;
        assert!(
            gone,
            "stdin reader did not observe EOF after writer cleanup"
        );
        assert!(
            drained.unwrap() < 4 * 1024 * 1024,
            "detached writer continued after run teardown"
        );
        assert!(unrelated_alive, "run teardown killed an unrelated process");
        if drop_run {
            assert!(result.is_none());
        } else {
            assert!(matches!(result, Some(Err(ProcessError::Cancelled { .. }))));
        }
    }
}

#[tokio::test]
async fn exit_codes_and_separate_streams() {
    let result = NativeProcessRunner
        .run(&sh("echo out; echo err >&2; exit 7"), RunOptions::default())
        .await
        .unwrap();
    assert_eq!(result.exit_code, 7);
    assert_eq!(result.stdout, b"out\n");
    assert_eq!(result.stderr, b"err\n");
    assert!(result.output.is_none());
    let error = result.require_success().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Command failed (exit 7): /bin/sh -c echo out; echo err >&2; exit 7: err"
    );
}

#[tokio::test]
async fn combined_capture_is_bounded_and_flags_truncation() {
    let options = RunOptions {
        combine_output: true,
        max_output_bytes: Some(10),
        ..RunOptions::default()
    };
    let result = NativeProcessRunner
        .run(&sh("printf 0123456789abcdef"), options)
        .await
        .unwrap();
    assert_eq!(result.output.as_deref(), Some(&b"0123456789"[..]));
    assert!(result.output_truncated);
    let exact = RunOptions {
        combine_output: true,
        max_output_bytes: Some(4),
        ..RunOptions::default()
    };
    let result = NativeProcessRunner
        .run(&sh("printf abcd"), exact)
        .await
        .unwrap();
    assert!(!result.output_truncated);
}

#[tokio::test]
async fn cwd_env_stdin_and_sink() {
    let temp = tempfile::tempdir().unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let options = RunOptions {
        stdin: Some(b"piped".to_vec()),
        output_sink: Some(sender),
        ..RunOptions::default()
    };
    let command = sh("pwd; echo $FOO; cat").cwd(temp.path()).env("FOO", "bar");
    let collect = tokio::spawn(async move {
        let mut all = Vec::new();
        while let Some(chunk) = receiver.recv().await {
            all.extend(chunk);
        }
        all
    });
    let result = NativeProcessRunner.run(&command, options).await.unwrap();
    let text = String::from_utf8(result.stdout).unwrap();
    let canonical = std::fs::canonicalize(temp.path()).unwrap();
    assert!(text.starts_with(&canonical.to_string_lossy().to_string()));
    assert!(text.ends_with("bar\npiped"));
    assert_eq!(String::from_utf8(collect.await.unwrap()).unwrap(), text);
}

#[tokio::test]
async fn missing_cwd_and_program_fail_before_spawn() {
    let error = NativeProcessRunner
        .run(
            &sh("true").cwd("/definitely/not/here"),
            RunOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProcessError::Platform(_)));
    let error = NativeProcessRunner
        .run(
            &Command::new("opencode-no-such-binary"),
            RunOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProcessError::Platform(_)));
}

#[tokio::test]
async fn timeout_kills_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pid");
    let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
    let options = RunOptions {
        timeout: Some(Duration::from_millis(300)),
        kill_grace: Some(Duration::from_millis(200)),
        ..RunOptions::default()
    };
    let started = Instant::now();
    let error = NativeProcessRunner
        .run(&sh(&script), options)
        .await
        .unwrap_err();
    assert!(matches!(error, ProcessError::TimedOut { .. }));
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(wait_gone(pid).await, "descendant {pid} survived timeout");
}

#[tokio::test]
async fn sigterm_ignoring_group_is_escalated_to_sigkill() {
    let options = RunOptions {
        timeout: Some(Duration::from_millis(200)),
        kill_grace: Some(Duration::from_millis(200)),
        ..RunOptions::default()
    };
    let started = Instant::now();
    let error = NativeProcessRunner
        .run(&sh("trap '' TERM; sleep 30"), options)
        .await
        .unwrap_err();
    assert!(matches!(error, ProcessError::TimedOut { .. }));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn cancellation_kills_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pid");
    let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let options = RunOptions {
        cancel: Some(cancel),
        kill_grace: Some(Duration::from_millis(200)),
        ..RunOptions::default()
    };
    let error = NativeProcessRunner
        .run(&sh(&script), options)
        .await
        .unwrap_err();
    assert!(matches!(error, ProcessError::Cancelled { .. }));
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(wait_gone(pid).await, "descendant {pid} survived cancel");
}

#[tokio::test]
async fn dropping_a_run_kills_the_group() {
    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pid");
    let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
    let command = sh(&script);
    let run = NativeProcessRunner.run(&command, RunOptions::default());
    let _ = tokio::time::timeout(Duration::from_millis(300), run).await;
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(wait_gone(pid).await, "descendant {pid} survived drop");
}

#[tokio::test]
async fn stream_lines_and_exit_check() {
    let lines: Vec<_> = NativeProcessRunner
        .run_stream(sh("echo a; echo; echo b"), StreamOptions::default())
        .collect()
        .await;
    let lines: Vec<String> = lines.into_iter().map(Result::unwrap).collect();
    assert_eq!(lines, ["a", "b"]);
    let options = StreamOptions {
        ok_exit_codes: vec![0],
        ..StreamOptions::default()
    };
    let items: Vec<_> = NativeProcessRunner
        .run_stream(sh("echo a; echo boom >&2; exit 3"), options)
        .collect()
        .await;
    assert_eq!(items[0].as_deref().unwrap(), "a");
    let error = items[1].as_ref().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Command failed (exit 3): /bin/sh -c echo a; echo boom >&2; exit 3: boom"
    );
}
