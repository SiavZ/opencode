//! Purpose: contract tests for platform adapters: loopback NO_PROXY upsert, env proxy selection and
//! client construction, shell-env parsing/merging/probing (port of shell-env.test.ts plus a live
//! login-shell probe), app detection, Windows shim resolution and open-path failure typing.
//! Scope: integration tests through `opencode_desktop_host::platform::{proxy,shell_env,apps}`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/apps.rs
//! |-- desktop-host/src/platform/proxy.rs
//! `-- desktop-host/src/platform/shell_env.rs
//! External:
//! |-- axum::Router
//! |-- tempfile::tempdir
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: never opens real user-visible apps or files; probes run against temp scripts.
//! Contract tests: desktop-host/tests/platform.rs

use std::path::Path;
use std::time::Duration;

use opencode_desktop_host::platform::apps::{
    AppsError, Os, check_app_exists, open_path, resolve_app_path, resolve_windows_candidates,
};
use opencode_desktop_host::platform::proxy::{
    EnvMap, EnvProxy, ensure_loopback_no_proxy, http_client, upsert_loopback,
};
use opencode_desktop_host::platform::shell_env::{
    Probe, is_nushell, load_shell_env, merge_shell_env, parse_shell_env, prefer_app_env, probe,
    resolve_user_shell,
};

fn env(pairs: &[(&str, &str)]) -> EnvMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// --- proxy (index.ts:95-113, sidecar.ts:83-101) ---

#[test]
fn loopback_hosts_are_appended_once_case_insensitively() {
    assert_eq!(upsert_loopback(None), "127.0.0.1,localhost,::1");
    assert_eq!(
        upsert_loopback(Some(" corp.local , LOCALHOST ,,")),
        "corp.local,LOCALHOST,127.0.0.1,::1"
    );
    let mut map = env(&[("no_proxy", "a")]);
    ensure_loopback_no_proxy(&mut map);
    assert_eq!(map["no_proxy"], "a,127.0.0.1,localhost,::1");
    assert_eq!(map["NO_PROXY"], "127.0.0.1,localhost,::1");
}

#[test]
fn env_proxy_prefers_lowercase_and_ignores_empty() {
    let proxy = EnvProxy::from_env(&env(&[
        ("http_proxy", "http://lower:1"),
        ("HTTP_PROXY", "http://upper:1"),
        ("https_proxy", ""),
        ("HTTPS_PROXY", "http://secure:2"),
        ("NO_PROXY", "x"),
    ]));
    assert_eq!(proxy.http.as_deref(), Some("http://lower:1"));
    assert_eq!(proxy.https.as_deref(), Some("http://secure:2"));
    assert_eq!(proxy.no_proxy.as_deref(), Some("x"));
}

#[tokio::test]
async fn http_client_routes_through_env_proxy_but_not_for_no_proxy_hosts() {
    // A one-route "proxy": any absolute-form request it receives returns 299.
    let app = axum::Router::new()
        .fallback(|| async { (axum::http::StatusCode::from_u16(299).unwrap(), "proxied") });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = http_client(&env(&[
        ("HTTP_PROXY", &proxy),
        ("NO_PROXY", "skip.invalid"),
    ]))
    .unwrap();
    let response = client
        .get("http://upstream.invalid/x")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 299);
    // NO_PROXY host bypasses the proxy and fails DNS directly.
    assert!(client.get("http://skip.invalid/x").send().await.is_err());
    assert!(http_client(&env(&[("HTTP_PROXY", "::not a url::")])).is_err());
    assert!(http_client(&EnvMap::new()).is_ok());
}

// --- shell env (port of shell-env.test.ts) ---

#[test]
fn parse_shell_env_supports_null_delimited_pairs() {
    let parsed = parse_shell_env(b"PATH=/usr/bin:/bin\0FOO=bar=baz\0\0");
    assert_eq!(parsed["PATH"], "/usr/bin:/bin");
    assert_eq!(parsed["FOO"], "bar=baz");
}

#[test]
fn parse_shell_env_ignores_invalid_entries() {
    let parsed = parse_shell_env(b"INVALID\0=empty\0OK=1\0");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed["OK"], "1");
}

#[test]
fn merge_shell_env_keeps_explicit_overrides() {
    let merged = merge_shell_env(
        Some(&env(&[("PATH", "/shell/path"), ("HOME", "/tmp/home")])),
        &env(&[("PATH", "/desktop/path"), ("OPENCODE_CLIENT", "desktop")]),
    );
    assert_eq!(merged["PATH"], "/desktop/path");
    assert_eq!(merged["HOME"], "/tmp/home");
    assert_eq!(merged["OPENCODE_CLIENT"], "desktop");
}

#[test]
fn resolve_user_shell_falls_back_to_login_shell_before_bin_sh() {
    assert_eq!(
        resolve_user_shell(Some("/custom/env-shell"), Some("/bin/zsh")),
        "/custom/env-shell"
    );
    assert_eq!(resolve_user_shell(None, Some("/bin/zsh")), "/bin/zsh");
    assert_eq!(resolve_user_shell(None, Some("unknown")), "/bin/sh");
    assert_eq!(resolve_user_shell(None, None), "/bin/sh");
}

#[test]
fn is_nushell_handles_path_and_binary_name() {
    assert!(is_nushell("nu"));
    assert!(is_nushell("/opt/homebrew/bin/nu"));
    assert!(is_nushell("C:\\Program Files\\nu.exe"));
    assert!(!is_nushell("/bin/zsh"));
}

#[test]
fn prefer_app_env_sets_desktop_flags_and_state_home() {
    let out = prefer_app_env(
        &env(&[("PATH", "/app")]),
        Some(&env(&[("PATH", "/shell"), ("EDITOR", "vim")])),
        Path::new("/user/data"),
    );
    assert_eq!(out["PATH"], "/shell");
    assert_eq!(out["EDITOR"], "vim");
    assert_eq!(out["OPENCODE_CLIENT"], "desktop");
    assert_eq!(out["OPENCODE_EXPERIMENTAL_ICON_DISCOVERY"], "true");
    assert_eq!(out["OPENCODE_EXPERIMENTAL_FILEWATCHER"], "true");
    assert_eq!(out["XDG_STATE_HOME"], "/user/data");
    let kept = prefer_app_env(&env(&[("XDG_STATE_HOME", "/s")]), None, Path::new("/u"));
    assert_eq!(kept["XDG_STATE_HOME"], "/s");
}

#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().to_string()
}

#[cfg(unix)]
#[tokio::test]
async fn live_login_shell_probe_captures_env() {
    // Real /bin/sh login probe: `sh -l -c 'env -0'` (sh rejects -il on some systems, so -l fallback).
    let env = load_shell_env("/bin/sh", Duration::from_secs(5))
        .await
        .expect("sh login probe returns an env");
    assert!(env.contains_key("PATH"));
}

#[cfg(unix)]
#[tokio::test]
async fn shell_probe_falls_back_to_l_and_gives_up_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    // Fails for -il, succeeds for -l.
    let fallback = script(
        dir.path(),
        "fallback",
        r#"[ "$1" = "-il" ] && exit 1; printf 'FROM=l\0'"#,
    );
    assert_eq!(
        load_shell_env(&fallback, Duration::from_secs(5)).await,
        Some(env(&[("FROM", "l")]))
    );
    let hangs = script(dir.path(), "hangs", "sleep 10");
    assert_eq!(
        probe(&hangs, "-il", Duration::from_millis(200)).await,
        Probe::Timeout
    );
    // Timeout on -il gives up without trying -l.
    assert_eq!(
        load_shell_env(&hangs, Duration::from_millis(200)).await,
        None
    );
    let empty = script(dir.path(), "empty", "exit 0");
    assert_eq!(
        probe(&empty, "-l", Duration::from_secs(5)).await,
        Probe::Unavailable
    );
    assert_eq!(
        probe("/does/not/exist", "-l", Duration::from_secs(1)).await,
        Probe::Unavailable
    );
    assert_eq!(
        load_shell_env("/opt/bin/nu", Duration::from_secs(1)).await,
        None
    );
}

// --- apps (apps.ts) ---

#[tokio::test]
async fn app_detection_per_os() {
    assert!(check_app_exists(Os::Windows, "anything", None).await);
    assert!(check_app_exists(Os::Linux, "anything", None).await);
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("Applications/Fake Editor.app")).unwrap();
    assert!(check_app_exists(Os::Mac, "Fake Editor", Some(home.path())).await);
    assert!(!check_app_exists(Os::Mac, "Definitely Missing App 42", Some(home.path())).await);
    // `which` fallback finds CLI tools.
    assert!(check_app_exists(Os::Mac, "sh", None).await);
    assert_eq!(
        resolve_app_path(Os::Mac, "code").await.as_deref(),
        Some("code")
    );
    assert_eq!(
        resolve_app_path(Os::Linux, "code").await.as_deref(),
        Some("code")
    );
}

#[tokio::test]
async fn windows_candidate_resolution_follows_cmd_shims() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let s = |p: &Path| p.to_string_lossy().to_string();
    // .exe wins outright.
    let paths = vec![s(&root.join("a.cmd")), s(&root.join("Code.exe"))];
    assert_eq!(
        resolve_windows_candidates("code", &paths).await,
        Some(s(&root.join("Code.exe")))
    );
    // .cmd shim with %~dp0 relative target.
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("Code.exe"), "").unwrap();
    std::fs::write(
        root.join("bin/code.cmd"),
        "@echo off\r\n\"%~dp0..\\Code.exe\" %*\r\n",
    )
    .unwrap();
    let shim = vec![s(&root.join("bin/code.cmd"))];
    assert_eq!(
        resolve_windows_candidates("code", &shim).await,
        Some(s(&root.join("Code.exe")))
    );
    // Extensionless path with a sibling .cmd.
    let bare = vec![s(&root.join("bin/code"))];
    assert_eq!(
        resolve_windows_candidates("code", &bare).await,
        Some(s(&root.join("Code.exe")))
    );
    // Directory scan for a matching exe within three parents.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(other.path().join("x/y")).unwrap();
    std::fs::write(other.path().join("Cursor-App.exe"), "").unwrap();
    let scan = vec![s(&other.path().join("x/y/cursor"))];
    assert_eq!(
        resolve_windows_candidates("cursor app", &scan).await,
        Some(s(&other.path().join("Cursor-App.exe")))
    );
    // Nothing better: first path.
    let none = vec!["/nowhere/a".to_string(), "/nowhere/b".to_string()];
    assert_eq!(
        resolve_windows_candidates("zz", &none).await,
        Some("/nowhere/a".into())
    );
    assert_eq!(resolve_windows_candidates("zz", &[]).await, None);
}

#[tokio::test]
async fn open_path_failures_are_typed() {
    let missing = open_path(Os::Linux, "/tmp", Some("/does/not/exist/app")).await;
    assert!(matches!(missing, Err(AppsError::Spawn { .. })));
    #[cfg(unix)]
    {
        let failing = open_path(Os::Linux, "/tmp", Some("false")).await;
        assert!(matches!(
            failing,
            Err(AppsError::Exit { code: Some(1), .. })
        ));
        assert!(open_path(Os::Linux, "/tmp", Some("true")).await.is_ok());
    }
}
