//! Purpose: contract tests for connection identity, sidecar env/port helpers, health checks with
//! basic auth, and background CLI staging/discovery against a real scripted fake CLI process.
//! Scope: integration tests through `opencode_desktop_host::connections` (temp dirs only).
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/background_cli.rs
//! |-- desktop-host/src/connections/sidecar.rs
//! `-- desktop-host/src/platform/proxy.rs
//! External:
//! |-- axum::Router::new
//! |-- axum::http::HeaderMap
//! |-- axum::http::StatusCode
//! |-- axum::routing::get
//! |-- axum::serve
//! |-- reqwest::Client::new
//! |-- serde_json::to_string
//! |-- str::to_string
//! |-- tempfile::tempdir
//! |-- tokio::net::TcpListener::bind
//! |-- tokio::spawn
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data; fake CLI is a real executable shell script (unix).
//! Contract tests: desktop-host/tests/connections.rs

use std::path::Path;

use opencode_desktop_host::connections::background_cli::{
    BackgroundCliConfig, PasswordCommand, executable_name, service_url, stage_cli,
    start_background_cli, state_home_candidates, version_dir,
};
use opencode_desktop_host::connections::sidecar::{
    allocate_port, new_password, port_from_env, serve_args, sidecar_env,
};
use opencode_desktop_host::connections::{
    BasicAuth, ConnectionError, ServerConnection, ServerReadyData, SidecarSlot, check_health,
};
use opencode_desktop_host::platform::proxy::EnvMap;

fn ready(url: &str, password: Option<&str>) -> ServerReadyData {
    ServerReadyData {
        url: url.into(),
        username: password.map(|_| "opencode".into()),
        password: password.map(str::to_string),
    }
}

fn env(pairs: &[(&str, &str)]) -> EnvMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn connection_keys_and_ownership_follow_server_tsx() {
    let daemon = ServerConnection::Daemon(ready("http://127.0.0.1:4096", Some("pw")));
    let remote = ServerConnection::Remote(ready("https://example.test", None));
    let wsl = ServerConnection::Wsl {
        distro: "Debian".into(),
        ready: ready("http://127.0.0.1:5000", Some("x")),
    };
    assert_eq!(daemon.key(), "sidecar");
    assert_eq!(remote.key(), "https://example.test");
    assert_eq!(wsl.key(), "wsl:Debian");
    assert!(!daemon.owned_by_host());
    assert!(!remote.owned_by_host());
    assert!(!wsl.owned_by_host());
    assert_eq!(
        daemon.basic_auth(),
        Some(BasicAuth {
            username: "opencode".into(),
            password: "pw".into()
        })
    );
    assert_eq!(remote.basic_auth(), None);
    let json = serde_json::to_string(daemon.ready_data()).unwrap();
    assert_eq!(
        json,
        r#"{"url":"http://127.0.0.1:4096","username":"opencode","password":"pw"}"#
    );
}

#[tokio::test]
async fn kill_sidecar_clears_but_never_stops_non_owned_connections() {
    let slot = SidecarSlot::new();
    assert!(slot.kill_sidecar().await.is_none());
    slot.set(ServerConnection::Daemon(ready(
        "http://127.0.0.1:1",
        Some("p"),
    )))
    .await;
    assert!(slot.kill_sidecar().await.is_none());
    assert!(slot.current().await.is_none());
}

#[test]
fn opencode_port_follows_parse_int_semantics() {
    assert_eq!(port_from_env(&env(&[])).unwrap(), None);
    assert_eq!(port_from_env(&env(&[("OPENCODE_PORT", "")])).unwrap(), None);
    assert_eq!(
        port_from_env(&env(&[("OPENCODE_PORT", "4096")])).unwrap(),
        Some(4096)
    );
    assert_eq!(
        port_from_env(&env(&[("OPENCODE_PORT", "  4097abc")])).unwrap(),
        Some(4097)
    );
    assert_eq!(
        port_from_env(&env(&[("OPENCODE_PORT", "abc")])).unwrap(),
        None
    );
    assert!(matches!(
        port_from_env(&env(&[("OPENCODE_PORT", "70000")])),
        Err(ConnectionError::Port(_))
    ));
    assert!(matches!(
        port_from_env(&env(&[("OPENCODE_PORT", "-1")])),
        Err(ConnectionError::Port(_))
    ));
}

#[tokio::test]
async fn allocates_a_free_loopback_port() {
    let port = allocate_port("127.0.0.1").await.unwrap();
    assert!(port > 0);
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("allocated port is released for the sidecar");
}

#[test]
fn sidecar_env_sets_auth_state_home_and_loopback_no_proxy() {
    let base = env(&[
        ("DEBUG", "1"),
        ("LD_PRELOAD", "/x.so"),
        ("NO_PROXY", "corp.local"),
        ("PATH", "/bin"),
    ]);
    let out = sidecar_env(&base, "pw", Path::new("/data"), true);
    assert_eq!(out.get("DEBUG"), None);
    assert_eq!(out.get("LD_PRELOAD"), None);
    assert_eq!(out["OPENCODE_SERVER_USERNAME"], "opencode");
    assert_eq!(out["OPENCODE_SERVER_PASSWORD"], "pw");
    assert_eq!(out["XDG_STATE_HOME"], "/data");
    assert_eq!(out["OPENCODE_DISABLE_EMBEDDED_WEB_UI"], "true");
    assert_eq!(out["NO_PROXY"], "corp.local,127.0.0.1,localhost,::1");
    assert_eq!(out["no_proxy"], "127.0.0.1,localhost,::1");
    let mac = sidecar_env(
        &env(&[("LD_PRELOAD", "/x.so"), ("XDG_STATE_HOME", "/s")]),
        "pw",
        Path::new("/data"),
        false,
    );
    assert_eq!(mac["LD_PRELOAD"], "/x.so");
    assert_eq!(mac["XDG_STATE_HOME"], "/s");
}

#[test]
fn serve_args_and_password_shape() {
    assert_eq!(
        serve_args("127.0.0.1", 4096),
        [
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            "4096",
            "--cors",
            "oc://renderer"
        ]
    );
    let password = new_password();
    assert_eq!(password.len(), 36);
    assert_ne!(password, new_password());
}

async fn auth_server() -> String {
    use axum::http::{HeaderMap, StatusCode};
    let app = axum::Router::new().route(
        "/global/health",
        axum::routing::get(|headers: HeaderMap| async move {
            // base64("opencode:secret")
            let ok = headers
                .get("authorization")
                .is_some_and(|value| value == "Basic b3BlbmNvZGU6c2VjcmV0");
            if ok {
                return StatusCode::OK;
            }
            StatusCode::UNAUTHORIZED
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn health_falls_back_to_global_health_with_basic_auth() {
    let url = auth_server().await;
    let client = reqwest::Client::new();
    let auth = BasicAuth::opencode("secret");
    assert!(check_health(&client, &url, auth.as_ref()).await);
    assert!(!check_health(&client, &url, BasicAuth::opencode("wrong").as_ref()).await);
    assert!(!check_health(&client, &url, None).await);
    assert!(!check_health(&client, "not a url", None).await);
    assert_eq!(BasicAuth::opencode(""), None);
}

#[test]
fn background_cli_pure_helpers() {
    assert_eq!(executable_name(true), "opencode-cli.exe");
    assert_eq!(executable_name(false), "opencode-cli");
    assert_eq!(
        service_url("http://127.0.0.1:4096"),
        Some("http://127.0.0.1:4096".into())
    );
    assert_eq!(
        service_url("running http://127.0.0.1:4096"),
        Some("http://127.0.0.1:4096".into())
    );
    assert_eq!(service_url("stopped"), None);
    assert_eq!(version_dir("1.2.3-beta+abc/x"), "1.2.3-beta-abc-x");
    assert_eq!(PasswordCommand::default(), PasswordCommand::DesktopBaseline);
    assert_eq!(
        PasswordCommand::DesktopBaseline.args(),
        ["service", "get", "password"]
    );
    assert_eq!(PasswordCommand::CliSpec.args(), ["service", "password"]);
}

#[test]
fn state_home_candidates_dedupe_and_keep_only_existing_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let dev = dir.path().join("ai.opencode.desktop.dev");
    std::fs::create_dir_all(&dev).unwrap();
    let dev = dev.to_string_lossy().to_string();
    let candidates = state_home_candidates(None, Some(&dev), dir.path());
    assert_eq!(candidates, vec![None, Some(dev)]);
    let empty = tempfile::tempdir().unwrap();
    let missing = state_home_candidates(Some("/does/not/exist"), None, empty.path());
    assert_eq!(missing, vec![None]);
}

#[tokio::test]
async fn stage_cli_copies_once_and_reuses() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("src-cli");
    std::fs::write(&source, "v1").unwrap();
    let staged = stage_cli(&source, dir.path(), "1.0.0", false)
        .await
        .unwrap();
    assert_eq!(staged, dir.path().join("cli/1.0.0/opencode-cli"));
    assert_eq!(std::fs::read_to_string(&staged).unwrap(), "v1");
    std::fs::write(&source, "v2").unwrap();
    let again = stage_cli(&source, dir.path(), "1.0.0", false)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(again).unwrap(), "v1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

/// Fake CLI: records each invocation's args and XDG_STATE_HOME, reports a running daemon only for
/// `$RUNNING_HOME`, and accepts only the password vocabulary in `$PASSWORD_VOCAB`.
#[cfg(unix)]
fn fake_cli(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("opencode-cli");
    let script = r#"#!/bin/sh
echo "$* | ${XDG_STATE_HOME-<unset>}" >> "$LOG"
case "$*" in
  --version) echo "9.9.9" ;;
  "service status")
    if [ "${XDG_STATE_HOME-}" = "$RUNNING_HOME" ]; then echo "running http://127.0.0.1:7777"; else echo "stopped"; fi ;;
  "service start") echo "http://127.0.0.1:7777" ;;
  "service get password") [ "$PASSWORD_VOCAB" = baseline ] && echo "s3cret" || { echo "unknown command" >&2; exit 2; } ;;
  "service password") [ "$PASSWORD_VOCAB" = spec ] && echo "s3cret" || { echo "unknown command" >&2; exit 2; } ;;
  *) exit 9 ;;
esac
"#;
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn cli_config(dir: &Path, vocab: &str, command: PasswordCommand) -> BackgroundCliConfig {
    let app_data = dir.join("appdata");
    let running = app_data.join("ai.opencode.desktop.beta");
    std::fs::create_dir_all(&running).unwrap();
    BackgroundCliConfig {
        bundled: fake_cli(dir),
        packaged: true,
        user_data: dir.join("userdata"),
        app_data,
        env: env(&[
            ("PATH", "/usr/bin:/bin"),
            ("LOG", &dir.join("log").to_string_lossy()),
            ("RUNNING_HOME", &running.to_string_lossy()),
            ("PASSWORD_VOCAB", vocab),
        ]),
        shell_state_home: None,
        password_command: command,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn background_cli_stages_discovers_existing_daemon_and_reads_password() {
    let dir = tempfile::tempdir().unwrap();
    let config = cli_config(dir.path(), "baseline", PasswordCommand::DesktopBaseline);
    let connection = start_background_cli(&config).await.unwrap();
    assert!(!connection.owned_by_host());
    let ServerConnection::Daemon(ready) = connection else {
        panic!("daemon expected");
    };
    assert_eq!(ready.url, "http://127.0.0.1:7777");
    assert_eq!(ready.password.as_deref(), Some("s3cret"));
    assert!(dir.path().join("userdata/cli/9.9.9/opencode-cli").exists());
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    let beta = dir.path().join("appdata/ai.opencode.desktop.beta");
    let beta = beta.to_string_lossy();
    assert!(log.starts_with("--version | <unset>\n"), "{log}");
    assert!(log.contains(&format!("service start | {beta}")), "{log}");
    assert!(
        log.contains(&format!("service get password | {beta}")),
        "{log}"
    );
}

/// Blocker evidence: the desktop baseline asks for `service get password`, the CLI spec (and the
/// Rust CLI port) only declares `service password`. Against a spec-conformant CLI, baseline startup
/// fails with a typed command error; the `CliSpec` vocabulary succeeds.
#[cfg(unix)]
#[tokio::test]
async fn password_vocabulary_discrepancy_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    let config = cli_config(dir.path(), "spec", PasswordCommand::DesktopBaseline);
    let error = start_background_cli(&config).await.unwrap_err();
    let ConnectionError::Command {
        command, stderr, ..
    } = error
    else {
        panic!("command error expected, got {error:?}");
    };
    assert!(command.ends_with("service get password"));
    assert_eq!(stderr, "unknown command");
    let dir = tempfile::tempdir().unwrap();
    let config = cli_config(dir.path(), "spec", PasswordCommand::CliSpec);
    assert!(start_background_cli(&config).await.is_ok());
}
