//! Purpose: tests for the WSL runtime helpers (argument building, output parsing, UTF-16LE decoding,
//! bootstrap script, opencode check) and the real `ExeRunner` process adapter. On unix the adapter
//! is exercised with real POSIX commands for timeout/cancel/exit-code behavior; the actual
//! `wsl.exe` path is `cfg(windows)` and blocked (no Windows runner).
//! Scope: integration tests through `opencode_desktop_host::platform::wsl::{runtime,exe,sidecar}`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/exe.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! |-- desktop-host/src/platform/wsl/runtime.rs
//! `-- desktop-host/src/platform/wsl/sidecar.rs
//! External:
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio::time::sleep
//! |-- tokio_util::sync::CancellationToken
//! `-- u16::to_le_bytes
//! Used by:
//! `-- none, entrypoint
//! Invariants: no wsl.exe invocation on non-Windows hosts.
//! Contract tests: desktop-host/tests/wsl_runtime.rs

use std::time::Duration;

use opencode_desktop_host::platform::wsl::controller::opencode_check;
use opencode_desktop_host::platform::wsl::exe::{ExeRunner, decode_output};
use opencode_desktop_host::platform::wsl::runtime::{
    RunOptions, WslError, WslRunner, first_line, install_opencode_script, parse_installed_distros,
    parse_online_distros, shell_escape, summarize, wsl_args,
};
use opencode_desktop_host::platform::wsl::sidecar::bootstrap_script;
use opencode_desktop_host::platform::wsl::{WslInstalledDistro, WslOnlineDistro};
use tokio_util::sync::CancellationToken;

#[test]
fn wsl_args_and_escaping() {
    assert_eq!(
        wsl_args(&["sh", "-lc", "x"], Some("Debian"), Some("root")),
        ["-d", "Debian", "--user", "root", "--", "sh", "-lc", "x"]
    );
    assert_eq!(wsl_args(&["true"], None, Some("")), ["--", "true"]);
    assert_eq!(shell_escape("it's"), "'it'\"'\"'s'");
    assert_eq!(summarize("\n  a \r\n\n b\n"), "a\nb");
    assert_eq!(first_line("\n\n  v1.2 \nnext"), Some("v1.2".into()));
    assert_eq!(first_line("  \n"), None);
    assert_eq!(
        install_opencode_script("1.16.2"),
        "curl -fsSL https://opencode.ai/install | bash -s -- --version '1.16.2'"
    );
}

#[test]
fn parses_installed_distros_like_the_baseline_regex() {
    let output = "  NAME                   STATE           VERSION\r\n* Ubuntu-24.04           Running         2\r\n  Debian Preview         Stopped         1\r\n  broken line\r\n\r\n";
    assert_eq!(
        parse_installed_distros(output),
        [
            WslInstalledDistro {
                name: "Ubuntu-24.04".into(),
                version: Some(2),
                is_default: true
            },
            WslInstalledDistro {
                name: "Debian Preview".into(),
                version: Some(1),
                is_default: false
            },
        ]
    );
}

#[test]
fn parses_online_distros_like_the_baseline_regex() {
    let output = "The following is a list of valid distributions.\r\n\r\nNAME                            FRIENDLY NAME\r\nUbuntu                          Ubuntu\r\nDebian                          Debian GNU/Linux\r\nkali-linux                      Kali Linux Rolling\r\n";
    assert_eq!(
        parse_online_distros(output),
        [
            WslOnlineDistro {
                name: "Ubuntu".into(),
                label: "Ubuntu".into()
            },
            WslOnlineDistro {
                name: "Debian".into(),
                label: "Debian GNU/Linux".into()
            },
            WslOnlineDistro {
                name: "kali-linux".into(),
                label: "Kali Linux Rolling".into()
            },
        ]
    );
}

#[test]
fn decodes_utf16le_wsl_output() {
    let text = "NAME  STATE\r\n* Debian";
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(decode_output(&bytes), text);
    let no_bom: Vec<u8> = "Debian".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(decode_output(&no_bom), "Debian");
    assert_eq!(decode_output(b"plain utf8"), "plain utf8");
}

#[test]
fn bootstrap_script_matches_wsl_sidecar_ts() {
    let script = bootstrap_script("/home/me/.opencode/bin/opencode", 4100, "p'w", true);
    let lines: Vec<&str> = script.lines().collect();
    assert_eq!(lines[0], "set -euo pipefail");
    assert!(lines.contains(&"export OPENCODE_SERVER_USERNAME='opencode'"));
    assert!(lines.contains(&"export OPENCODE_SERVER_PASSWORD='p'\"'\"'w'"));
    assert_eq!(
        *lines.last().unwrap(),
        "exec '/home/me/.opencode/bin/opencode' --print-logs --log-level WARN serve --hostname 0.0.0.0 --port 4100"
    );
    assert!(bootstrap_script("x", 1, "p", false).contains("--log-level INFO"));
}

#[test]
fn opencode_check_shapes() {
    let missing = opencode_check("Debian", None, None, "1.16.2");
    assert_eq!(
        missing.error.as_deref(),
        Some("opencode is not installed in this distro")
    );
    let broken = opencode_check("Debian", Some("/x".into()), None, "1.16.2");
    assert_eq!(
        broken.error.as_deref(),
        Some("opencode is installed but could not run")
    );
    let old = opencode_check("Debian", Some("/x".into()), Some("1.0.0".into()), "1.16.2");
    assert_eq!(old.matches_desktop, Some(false));
    assert_eq!(old.error, None);
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        serde_json::json!({"distro": "Debian", "resolvedPath": "/x", "version": "1.0.0", "expectedVersion": "1.16.2", "matchesDesktop": false, "error": null})
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exe_runner_reports_exit_code_and_output() {
    let runner = ExeRunner::default();
    let result = runner
        .run(
            "/bin/sh",
            vec!["-c".into(), "echo out; echo err >&2; exit 4".into()],
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.code, Some(4));
    assert_eq!(result.stdout, "out\n");
    assert_eq!(result.stderr, "err\n");
    assert!(matches!(
        runner
            .run("/does/not/exist", vec![], RunOptions::default())
            .await,
        Err(WslError::Spawn(_))
    ));
    assert_eq!(runner.system32("wsl.exe"), "wsl.exe");
}

#[cfg(unix)]
#[tokio::test]
async fn exe_runner_times_out_and_cancels() {
    let runner = ExeRunner::default();
    let started = std::time::Instant::now();
    let timeout = runner
        .run(
            "sleep",
            vec!["10".into()],
            RunOptions {
                cancel: None,
                timeout: Some(Duration::from_millis(200)),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(timeout.to_string(), "sleep 10 timed out after 200ms");
    assert!(started.elapsed() < Duration::from_secs(5));

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
    });
    let aborted = runner
        .run("sleep", vec!["10".into()], RunOptions::with_cancel(&cancel))
        .await
        .unwrap_err();
    assert_eq!(aborted, WslError::Aborted);
}

/// Real wsl.exe probe. Blocked: no Windows runner is available to the port (notes/connections.md).
#[cfg(windows)]
#[tokio::test]
#[ignore = "blocked: requires a Windows host with WSL (no runner)"]
async fn real_wsl_exe_probe_runtime() {
    use opencode_desktop_host::platform::wsl::runtime::probe_runtime;
    let runner = ExeRunner::from_env();
    let runtime = probe_runtime(&runner, &RunOptions::default()).await;
    assert!(runtime.available || runtime.error.is_some());
}
