//! Purpose: contract tests for env-derived startup configuration (`main/index.ts:53-140,191-196,
//! 350-375`, `main/constants.ts`).
//! Scope: pure functions over env maps; the onboarding root test uses a tempdir.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lifecycle/config.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never reads or writes the real process environment.
//! Contract tests: this file

use std::path::Path;

use opencode_desktop_host::lifecycle::config::*;

fn env(pairs: &[(&str, &str)]) -> EnvMap {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn channel_parse_falls_back_to_dev() {
    assert_eq!(Channel::parse(Some("prod")), Channel::Prod);
    assert_eq!(Channel::parse(Some("beta")), Channel::Beta);
    assert_eq!(Channel::parse(Some("dev")), Channel::Dev);
    assert_eq!(Channel::parse(Some("PROD")), Channel::Dev);
    assert_eq!(Channel::parse(None), Channel::Dev);
}

#[test]
fn app_names_and_ids_match_baseline() {
    assert_eq!(Channel::Prod.app_name(), "OpenCode");
    assert_eq!(Channel::Beta.app_name(), "OpenCode Beta");
    assert_eq!(Channel::Dev.app_name(), "OpenCode Dev");
    assert_eq!(Channel::Prod.app_id(), "ai.opencode.desktop");
    assert_eq!(Channel::Beta.app_id(), "ai.opencode.desktop.beta");
    assert_eq!(Channel::Dev.app_id(), "ai.opencode.desktop.dev");
    assert!(updater_enabled(true, Channel::Prod));
    assert!(!updater_enabled(true, Channel::Dev));
    assert!(!updater_enabled(false, Channel::Beta));
}

#[test]
fn identity_uses_dev_when_unpackaged_and_onboarding_root_when_testing() {
    let app_data = Path::new("/appdata");
    let packaged = app_identity(true, Channel::Beta, app_data, None);
    assert_eq!(packaged.name, "OpenCode Beta");
    assert_eq!(
        packaged.user_data,
        app_data.join("ai.opencode.desktop.beta")
    );
    assert_eq!(packaged.session_data, None);

    let unpackaged = app_identity(false, Channel::Prod, app_data, None);
    assert_eq!(unpackaged.app_id, "ai.opencode.desktop.dev");

    let root = Path::new("/tmp/root");
    let test = app_identity(true, Channel::Prod, app_data, Some(root));
    assert_eq!(test.user_data, root.join("desktop"));
    assert_eq!(test.session_data, Some(root.join("session")));
}

#[test]
fn env_flags_require_exact_one() {
    assert_eq!(
        SidecarVersion::from_env(&env(&[("OPENCODE_SIDECAR_V2", "1")])),
        SidecarVersion::V2
    );
    assert_eq!(
        SidecarVersion::from_env(&env(&[("OPENCODE_SIDECAR_V2", "true")])),
        SidecarVersion::V1
    );
    assert_eq!(SidecarVersion::from_env(&env(&[])), SidecarVersion::V1);
    assert!(test_onboarding(&env(&[("OPENCODE_TEST_ONBOARDING", "1")])));
    assert!(!test_onboarding(&env(&[(
        "OPENCODE_TEST_ONBOARDING",
        "yes"
    )])));
}

#[test]
fn onboarding_root_creates_isolated_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = create_onboarding_test_root(tmp.path()).expect("create");
    assert!(root.root.starts_with(tmp.path()));
    let name = root
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("opencode-onboarding-"));
    for dir in ONBOARDING_TEST_DIRS {
        assert!(root.root.join(dir).is_dir(), "{dir}");
    }
    assert_eq!(root.env["OPENCODE_DB"], ":memory:");
    assert_eq!(
        root.env["XDG_STATE_HOME"],
        root.root.join("state").to_string_lossy()
    );
    assert_eq!(root.env.len(), 5);
    assert_eq!(
        base_env_overrides()["OPENCODE_DISABLE_EMBEDDED_WEB_UI"],
        "true"
    );
}

#[test]
fn loopback_no_proxy_is_appended_to_both_keys() {
    let mut map = env(&[]);
    ensure_loopback_no_proxy(&mut map);
    assert_eq!(map["NO_PROXY"], "127.0.0.1,localhost,::1");
    assert_eq!(map["no_proxy"], "127.0.0.1,localhost,::1");

    let mut map = env(&[
        ("NO_PROXY", " corp.example , LOCALHOST,,"),
        ("no_proxy", "::1"),
    ]);
    ensure_loopback_no_proxy(&mut map);
    assert_eq!(map["NO_PROXY"], "corp.example,LOCALHOST,127.0.0.1,::1");
    assert_eq!(map["no_proxy"], "::1,127.0.0.1,localhost");

    // Idempotent.
    let before = map.clone();
    ensure_loopback_no_proxy(&mut map);
    assert_eq!(map, before);
}

#[test]
fn proxy_env_resolution_and_bypass() {
    let mut map = env(&[
        ("HTTPS_PROXY", "http://upper:1"),
        ("https_proxy", "http://lower:2"),
        ("HTTP_PROXY", "http://plain:3"),
        ("NO_PROXY", ".internal.example,10.0.0.5:8080"),
    ]);
    ensure_loopback_no_proxy(&mut map);
    let proxy = proxy_from_env(&map);
    assert_eq!(proxy.https_proxy.as_deref(), Some("http://lower:2"));
    assert_eq!(
        proxy.proxy_for("https", "api.example.com", 443),
        Some("http://lower:2")
    );
    assert_eq!(
        proxy.proxy_for("http", "api.example.com", 80),
        Some("http://plain:3")
    );
    assert_eq!(proxy.proxy_for("http", "127.0.0.1", 4096), None);
    assert_eq!(proxy.proxy_for("http", "[::1]", 4096), None);
    assert_eq!(proxy.proxy_for("http", "localhost", 1), None);
    // Baseline quirk: Node reads `no_proxy || NO_PROXY`, and ensureLoopbackNoProxy always writes a
    // non-empty lowercase `no_proxy`, so uppercase-only user entries are shadowed.
    assert_eq!(
        proxy.proxy_for("https", "svc.internal.example", 443),
        Some("http://lower:2")
    );
    assert_eq!(
        proxy.proxy_for("http", "10.0.0.5", 8080),
        Some("http://plain:3")
    );
    assert_eq!(
        proxy_from_env(&env(&[("NO_PROXY", "*"), ("HTTP_PROXY", "http://p")]))
            .proxy_for("http", "x", 1),
        None
    );
    assert_eq!(proxy_from_env(&env(&[])).proxy_for("https", "x", 1), None);
}

#[test]
fn no_proxy_matching_follows_node_proxy_config() {
    let proxy = proxy_from_env(&env(&[
        ("http_proxy", "http://p:1"),
        ("https_proxy", ""),
        ("HTTPS_PROXY", "socks5://ignored"),
        (
            "no_proxy",
            ".internal.example, 10.0.0.5:8080 ,*.wild.example,exact.example,192.168.1.0-192.168.1.255",
        ),
    ]));
    assert_eq!(
        proxy.https_proxy, None,
        "empty lowercase falls through, non-http proxy ignored"
    );
    assert_eq!(proxy.proxy_for("ws", "x", 1), None);
    assert_eq!(proxy.proxy_for("http", "svc.internal.example", 80), None);
    assert_eq!(proxy.proxy_for("http", "internal.example", 80), None);
    assert_eq!(
        proxy.proxy_for("http", "xinternal.example", 80),
        Some("http://p:1")
    );
    assert_eq!(proxy.proxy_for("http", "10.0.0.5", 8080), None);
    assert_eq!(proxy.proxy_for("http", "10.0.0.5", 80), Some("http://p:1"));
    assert_eq!(proxy.proxy_for("http", "a.wild.example", 80), None);
    assert_eq!(proxy.proxy_for("http", "exact.example", 80), None);
    assert_eq!(
        proxy.proxy_for("http", "sub.exact.example", 80),
        Some("http://p:1")
    );
    assert_eq!(proxy.proxy_for("http", "192.168.1.77", 80), None);
    assert_eq!(
        proxy.proxy_for("http", "192.168.2.1", 80),
        Some("http://p:1")
    );
}

#[test]
fn command_line_switches_match_baseline_order() {
    assert_eq!(
        command_line_switches(true, None),
        vec![
            ("proxy-bypass-list", "<-loopback>".to_string()),
            ("enable-features", JS_CALL_STACK_FEATURE.to_string()),
        ]
    );
    let dev = command_line_switches(false, Some("Foo,Bar"));
    assert_eq!(dev[1].1, format!("{JS_CALL_STACK_FEATURE},Foo,Bar"));
    assert_eq!(dev[2], ("remote-debugging-port", "9222".to_string()));
    assert_eq!(enable_features_switch(Some("")), JS_CALL_STACK_FEATURE);
}

#[test]
fn sidecar_port_uses_parse_int_semantics() {
    let port = |v: &str| sidecar_port_from_env(&env(&[("OPENCODE_PORT", v)]));
    assert_eq!(port("4096"), Some(4096));
    assert_eq!(port("  4096abc"), Some(4096));
    assert_eq!(port("+12"), Some(12));
    assert_eq!(port("-1"), Some(-1));
    assert_eq!(port("abc"), None);
    assert_eq!(port(""), None);
    assert_eq!(sidecar_port_from_env(&env(&[])), None);
    assert_eq!(local_sidecar_url(4096), "http://127.0.0.1:4096");
}
