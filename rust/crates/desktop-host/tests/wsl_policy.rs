//! Purpose: port of the pure-policy half of `packages/desktop/src/main/wsl/servers.test.ts`
//! (policy.ts/startup.ts helpers) and `renderer/wsl/connections.test.ts`, plus the WSL state wire
//! shape and persisted-server normalization.
//! Scope: fixture tests over pure functions; no processes.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! `-- desktop-host/src/platform/wsl/policy.rs
//! External:
//! |-- serde_json::from_value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: wire JSON matches `packages/app/src/wsl/types.ts` field names and key order.
//! Contract tests: desktop-host/tests/wsl_policy.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opencode_desktop_host::connections::ServerConnection;
use opencode_desktop_host::platform::wsl::policy::{
    available_startup_server, clear_wsl_distro_state, expect_opencode_version,
    normalize_persisted_servers, pending_restart_after_wsl_install, poll_health,
    ready_wsl_connections, require_ipc_string, require_ipc_strings, unavailable_state,
    wsl_server_id_to_restart, wsl_server_ids_to_start_on_initialize, wsl_terminal_args,
};
use opencode_desktop_host::platform::wsl::{
    Record, WslDistroProbe, WslJob, WslOpencodeCheck, WslRuntimeCheck, WslServerConfig,
    WslServerItem, WslServerRuntime, WslServersState,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn probe(name: &str) -> WslDistroProbe {
    WslDistroProbe {
        name: name.into(),
        can_execute: true,
        has_bash: true,
        has_curl: true,
        error: None,
    }
}

// ---------- policy (servers.test.ts:19-99) ----------

#[test]
fn starts_every_configured_wsl_server_on_initialization() {
    let servers = [
        WslServerConfig {
            id: "wsl:Debian".into(),
            distro: "Debian".into(),
        },
        WslServerConfig {
            id: "wsl:Ubuntu-24.04".into(),
            distro: "Ubuntu-24.04".into(),
        },
    ];
    assert_eq!(
        wsl_server_ids_to_start_on_initialize(&servers),
        ["wsl:Debian", "wsl:Ubuntu-24.04"]
    );
}

#[test]
fn rejects_an_update_that_did_not_install_the_desktop_version() {
    assert!(expect_opencode_version(Some("1.16.2"), "1.16.2", "Debian").is_ok());
    assert_eq!(
        expect_opencode_version(Some("1.14.35"), "1.16.2", "Debian").unwrap_err(),
        "OpenCode update finished but Debian still reports 1.14.35; expected 1.16.2"
    );
    assert_eq!(
        expect_opencode_version(None, "1.16.2", "Debian").unwrap_err(),
        "OpenCode update finished but Debian still reports no version; expected 1.16.2"
    );
}

#[test]
fn restarts_an_existing_distro_server_after_updating_opencode() {
    let servers = [WslServerItem {
        config: WslServerConfig {
            id: "wsl:Debian".into(),
            distro: "Debian".into(),
        },
        runtime: WslServerRuntime::Ready {
            url: String::new(),
            username: None,
            password: None,
        },
    }];
    assert_eq!(
        wsl_server_id_to_restart(&servers, "Debian").as_deref(),
        Some("wsl:Debian")
    );
    assert_eq!(wsl_server_id_to_restart(&[], "Debian"), None);
}

#[test]
fn clears_cached_distro_probes_when_removing_a_wsl_server() {
    let mut probes = Record::default();
    probes.insert("Debian".into(), probe("Debian"));
    let mut checks = Record::default();
    checks.insert(
        "Debian".into(),
        WslOpencodeCheck {
            distro: "Debian".into(),
            resolved_path: Some("/home/luke/.opencode/bin/opencode".into()),
            version: Some("1.16.2".into()),
            expected_version: Some("1.16.2".into()),
            matches_desktop: Some(true),
            error: None,
        },
    );
    let (probes, checks) = clear_wsl_distro_state(&probes, &checks, "Debian");
    assert!(probes.is_empty());
    assert!(checks.is_empty());
}

#[test]
fn opens_terminals_for_distro_names_containing_spaces() {
    assert_eq!(
        wsl_terminal_args(Some("Ubuntu Preview")),
        ["/c", "start", "", "wsl", "-d", "Ubuntu Preview"]
    );
    assert_eq!(wsl_terminal_args(None), ["/c", "start", "", "wsl"]);
}

#[tokio::test]
async fn stops_health_polling_when_sidecar_startup_settles() {
    let cancel = CancellationToken::new();
    let checks = Arc::new(AtomicUsize::new(0));
    let counter = checks.clone();
    let token = cancel.clone();
    let polling = tokio::spawn(async move {
        poll_health(
            || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { false }
            },
            &token,
            Duration::from_millis(1),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    cancel.cancel();
    polling.await.unwrap();
    let settled = checks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(checks.load(Ordering::SeqCst), settled);
}

#[test]
fn validates_wsl_ipc_identifiers_at_the_module_boundary() {
    assert_eq!(
        require_ipc_string("distro", &json!("Debian")).unwrap(),
        "Debian"
    );
    assert_eq!(
        require_ipc_strings("distro", &json!(["Debian", "Ubuntu"])).unwrap(),
        ["Debian", "Ubuntu"]
    );
    assert_eq!(
        require_ipc_string("distro", &json!(""))
            .unwrap_err()
            .to_string(),
        "Invalid distro"
    );
    assert_eq!(
        require_ipc_string("server id", &serde_json::Value::Null)
            .unwrap_err()
            .to_string(),
        "Invalid server id"
    );
    assert_eq!(
        require_ipc_strings("distro", &json!([]))
            .unwrap_err()
            .to_string(),
        "Invalid distro"
    );
}

#[test]
fn derives_a_required_windows_restart_from_the_post_install_runtime_probe() {
    assert!(pending_restart_after_wsl_install(&WslRuntimeCheck {
        available: false,
        version: None,
        error: Some("WSL unavailable".into()),
    }));
    assert!(!pending_restart_after_wsl_install(&WslRuntimeCheck {
        available: true,
        version: Some("WSL version: 2.6.1".into()),
        error: None,
    }));
}

// ---------- renderer/wsl/connections.test.ts ----------

fn renderer_state(runtime: WslServerRuntime) -> WslServersState {
    WslServersState {
        servers: vec![WslServerItem {
            config: WslServerConfig {
                id: "wsl:Debian".into(),
                distro: "Debian".into(),
            },
            runtime,
        }],
        ..WslServersState::default()
    }
}

fn ready_runtime() -> WslServerRuntime {
    WslServerRuntime::Ready {
        url: "http://127.0.0.1:4096".into(),
        username: Some("opencode".into()),
        password: Some("secret".into()),
    }
}

#[test]
fn publishes_a_wsl_server_only_after_it_reports_ready() {
    for runtime in [
        WslServerRuntime::Starting,
        WslServerRuntime::Failed {
            message: "boom".into(),
        },
        WslServerRuntime::Stopped,
    ] {
        assert!(ready_wsl_connections(Some(&renderer_state(runtime))).is_empty());
    }
    let ready = ready_wsl_connections(Some(&renderer_state(ready_runtime())));
    assert_eq!(ready.len(), 1);
    let ServerConnection::Wsl { distro, ready } = &ready[0] else {
        panic!("wsl connection expected");
    };
    assert_eq!(distro, "Debian");
    assert_eq!(ready.url, "http://127.0.0.1:4096");
    assert!(
        !ServerConnection::Wsl {
            distro: distro.clone(),
            ready: ready.clone()
        }
        .owned_by_host()
    );
}

#[test]
fn does_not_block_desktop_startup_on_a_configured_wsl_default() {
    let key = "wsl:Debian";
    assert_eq!(available_startup_server(Some(key), None), "sidecar");
    assert_eq!(
        available_startup_server(Some(key), Some(&renderer_state(WslServerRuntime::Starting))),
        "sidecar"
    );
    assert_eq!(
        available_startup_server(Some(key), Some(&renderer_state(ready_runtime()))),
        key
    );
    assert_eq!(available_startup_server(None, None), "sidecar");
    assert_eq!(
        available_startup_server(Some("http://remote"), None),
        "http://remote"
    );
}

#[test]
fn wire_shape_matches_typescript_state() {
    let mut state = renderer_state(ready_runtime());
    state.job = Some(WslJob::InstallDistro {
        distro: "Debian".into(),
        started_at: 1,
    });
    state.distro_probes.insert("Debian".into(), probe("Debian"));
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(
        value,
        json!({
            "runtime": null,
            "installed": [],
            "online": [],
            "distroProbes": {"Debian": {"name": "Debian", "canExecute": true, "hasBash": true, "hasCurl": true, "error": null}},
            "opencodeChecks": {},
            "pendingRestart": false,
            "servers": [{"config": {"id": "wsl:Debian", "distro": "Debian"}, "runtime": {"kind": "ready", "url": "http://127.0.0.1:4096", "username": "opencode", "password": "secret"}}],
            "job": {"kind": "install-distro", "distro": "Debian", "startedAt": 1}
        })
    );
    let back: WslServersState = serde_json::from_value(value).unwrap();
    assert_eq!(back, state);
    assert_eq!(
        serde_json::to_value(unavailable_state()).unwrap()["runtime"],
        json!({"available": false, "version": null, "error": "WSL is only available on Windows"})
    );
}

#[test]
fn normalizes_persisted_servers() {
    let servers = normalize_persisted_servers(&json!({
        "servers": [
            {"id": "wsl:Debian", "distro": "Debian"},
            {"distro": "Ubuntu"},
            {"id": "x", "distro": ""},
            {"id": "y"},
            "junk"
        ]
    }));
    assert_eq!(
        servers,
        [
            WslServerConfig {
                id: "wsl:Debian".into(),
                distro: "Debian".into()
            },
            WslServerConfig {
                id: "wsl:Ubuntu".into(),
                distro: "Ubuntu".into()
            }
        ]
    );
    assert!(normalize_persisted_servers(&json!(null)).is_empty());
}
