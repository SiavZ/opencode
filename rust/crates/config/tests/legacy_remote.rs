//! Purpose: remote/org/managed source tests derived from `packages/opencode/test/config/config.test.ts`
//! (:1673-1860 well-known config, remote_config tokens/headers, precedence, null config, invalid
//! payloads, HTML login pages; :2129-2230 parseManagedPlist; account org config), using a local
//! in-memory fixture transport. Also covers the typed `Unsupported` default transport.
//! Scope: integration tests, fixture-based (no network).
//! Uses:
//! rust/crates/
//! |-- config/src/legacy/load.rs
//! `-- config/src/legacy/remote.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: the fixture transport records requested URLs and Authorization headers only.
//! Contract tests: config/tests/legacy_remote.rs

use opencode_config::legacy::remote::parse_managed_plist;
use opencode_config::legacy::{
    self, AccountOrg, AccountSource, Instance, ManagedPreferences, NoAccounts, RemoteFetcher,
    RemoteResponse, Sources, UnsupportedRemote, WellKnownAuth,
};
use opencode_config::{ConfigError, Environment, RemoteError};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct Fixture {
    wellknown: Value,
    remote: Option<(String, String)>,
    seen: Mutex<Vec<(String, Option<String>)>>,
}

impl RemoteFetcher for Fixture {
    fn get(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<RemoteResponse, RemoteError> {
        self.seen
            .lock()
            .expect("lock")
            .push((url.to_string(), headers.get("Authorization").cloned()));
        let json = |body: String| RemoteResponse {
            status: 200,
            content_type: "application/json".into(),
            body,
        };
        if url.contains(".well-known/opencode") {
            return Ok(json(self.wellknown.to_string()));
        }
        match (&self.remote, url.contains("config.example.com")) {
            (Some((content_type, body)), true) => Ok(RemoteResponse {
                status: 200,
                content_type: content_type.clone(),
                body: body.clone(),
            }),
            _ => Ok(RemoteResponse {
                status: 404,
                content_type: "application/json".into(),
                body: "{}".into(),
            }),
        }
    }
}

struct Accounts {
    url: String,
    org: Option<AccountOrg>,
}

impl AccountSource for Accounts {
    fn wellknown(&self) -> Vec<WellKnownAuth> {
        if self.url.is_empty() {
            return Vec::new();
        }
        vec![WellKnownAuth {
            url: self.url.clone(),
            key: "TEST_TOKEN".into(),
            token: "test-token".into(),
        }]
    }
    fn active_org(&self) -> Result<Option<AccountOrg>, ConfigError> {
        Ok(self.org.clone())
    }
}

struct NoManaged;

impl ManagedPreferences for NoManaged {
    fn read(&self, _env: &Environment) -> Option<(String, String)> {
        None
    }
}

fn run(
    fixture: &Fixture,
    accounts: &dyn AccountSource,
    project: Option<Value>,
    env_vars: &[(&str, &str)],
) -> Result<Value, ConfigError> {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut env = Environment::isolated(tmp.path().join("home"));
    env.global_config = tmp.path().join("global");
    env.set_var(
        "OPENCODE_TEST_MANAGED_CONFIG_DIR",
        tmp.path().join("managed").display().to_string(),
    );
    for (key, value) in env_vars {
        env.set_var(key, *value);
    }
    let directory = tmp.path().join("project");
    std::fs::create_dir_all(&directory).expect("mkdir");
    if let Some(project) = project {
        std::fs::write(directory.join("opencode.json"), project.to_string()).expect("write");
    }
    let instance = Instance {
        directory: directory.clone(),
        worktree: directory,
    };
    let sources = Sources {
        accounts,
        remote: fixture,
        managed: &NoManaged,
    };
    legacy::load(&env, &instance, &sources).map(|state| state.config)
}

fn accounts(url: &str) -> Accounts {
    Accounts {
        url: url.into(),
        org: None,
    }
}

fn remote_json(value: Value) -> Option<(String, String)> {
    Some(("application/json".into(), value.to_string()))
}

#[test]
fn project_config_overrides_wellknown_and_trailing_slash_is_normalized() {
    let fixture = Fixture {
        wellknown: json!({ "config": { "mcp": { "jira": { "type": "remote", "url": "https://jira.example.com/mcp", "enabled": false } } } }),
        ..Default::default()
    };
    let project = json!({ "$schema": "x", "mcp": { "jira": { "type": "remote", "url": "https://jira.example.com/mcp", "enabled": true } } });
    let config = run(
        &fixture,
        &accounts("https://example.com/"),
        Some(project),
        &[],
    )
    .expect("load");
    assert_eq!(
        fixture.seen.lock().expect("lock")[0].0,
        "https://example.com/.well-known/opencode"
    );
    assert_eq!(config["mcp"]["jira"]["enabled"], json!(true));
}

#[test]
fn remote_config_templates_url_and_headers_without_touching_process_env() {
    let fixture = Fixture {
        wellknown: json!({
            "config": { "mcp": { "confluence": { "type": "remote", "url": "https://c.example.com/mcp", "enabled": false } } },
            "remote_config": { "url": "https://config.example.com/{env:TEST_TOKEN}/opencode.json", "headers": { "Authorization": "Bearer {env:TEST_TOKEN}" } },
        }),
        remote: remote_json(
            json!({ "config": { "mcp": { "confluence": { "type": "remote", "url": "https://c.example.com/mcp", "enabled": true } } } }),
        ),
        ..Default::default()
    };
    let project = json!({ "$schema": "x", "username": "{env:TEST_TOKEN}" });
    let config = run(
        &fixture,
        &accounts("https://example.com"),
        Some(project),
        &[("TEST_TOKEN", "preexisting-token")],
    )
    .expect("load");
    let seen = fixture.seen.lock().expect("lock").clone();
    assert_eq!(
        seen[1],
        (
            "https://config.example.com/test-token/opencode.json".into(),
            Some("Bearer test-token".into())
        )
    );
    assert_eq!(config["mcp"]["confluence"]["enabled"], json!(true));
    assert_eq!(
        config["username"],
        json!("test-token"),
        "auth env overlays project substitution"
    );
}

#[test]
fn null_wellknown_config_is_absent_and_bad_payloads_fail() {
    let ok = Fixture {
        wellknown: json!({ "config": null, "remote_config": { "url": "https://config.example.com/opencode.json" } }),
        remote: remote_json(
            json!({ "mcp": { "c": { "type": "remote", "url": "https://c", "enabled": true } } }),
        ),
        ..Default::default()
    };
    assert_eq!(
        run(&ok, &accounts("https://example.com"), None, &[]).expect("load")["mcp"]["c"]["enabled"],
        json!(true)
    );
    let bad = Fixture {
        remote: remote_json(json!("not an object")),
        ..ok
    };
    assert!(matches!(
        run(&bad, &accounts("https://example.com"), None, &[]),
        Err(ConfigError::Remote(RemoteError::Decode { .. }))
    ));
    let html = Fixture {
        wellknown: json!({ "remote_config": { "url": "https://config.example.com/opencode.json" } }),
        remote: Some((
            "text/html; charset=utf-8".into(),
            "<!DOCTYPE html><html>Login required</html>".into(),
        )),
        ..Default::default()
    };
    let error = run(&html, &accounts("https://example.com"), None, &[]).expect_err("auth");
    assert_eq!(
        error,
        ConfigError::RemoteAuth {
            url: "https://example.com".into(),
            remote: "https://config.example.com/opencode.json".into()
        }
    );
}

#[test]
fn unsupported_transport_is_typed_and_unused_without_wellknown_auth() {
    let tmp = tempfile::tempdir().expect("tmp");
    let env = Environment::isolated(tmp.path());
    let instance = Instance {
        directory: tmp.path().into(),
        worktree: tmp.path().into(),
    };
    let sources = Sources {
        accounts: &NoAccounts,
        remote: &UnsupportedRemote,
        managed: &NoManaged,
    };
    assert!(legacy::load(&env, &instance, &sources).is_ok());
    let accounts = accounts("https://example.com");
    let sources = Sources {
        accounts: &accounts,
        remote: &UnsupportedRemote,
        managed: &NoManaged,
    };
    let error = legacy::load(&env, &instance, &sources).expect_err("unsupported");
    assert!(
        matches!(error, ConfigError::Remote(RemoteError::Unsupported(url)) if url == "https://example.com/.well-known/opencode")
    );
}

#[test]
fn account_org_config_resolves_console_token_and_tracks_managed_providers() {
    let org = AccountOrg {
        url: "https://control.example.com".into(),
        config: Some(
            json!({ "provider": { "opencode": { "options": { "apiKey": "{env:OPENCODE_CONSOLE_TOKEN}" } } } }),
        ),
        token: Some("st_test_token".into()),
    };
    let accounts = Accounts {
        url: String::new(),
        org: Some(org),
    };
    let config = run(&Fixture::default(), &accounts, None, &[]).expect("load");
    assert_eq!(
        config["provider"]["opencode"]["options"]["apiKey"],
        json!("st_test_token")
    );
}

#[test]
fn parse_managed_plist_strips_mdm_metadata() {
    let input = json!({ "PayloadDisplayName": "OpenCode", "PayloadIdentifier": "ai.opencode.managed.test", "PayloadType": "ai.opencode.managed",
        "PayloadUUID": "AAAA", "PayloadVersion": 1, "_manualProfile": true, "share": "disabled", "server": { "hostname": "127.0.0.1", "mdns": false },
        "permission": { "*": "ask", "bash": { "*": "ask", "rm -rf *": "deny" } }, "enabled_providers": ["anthropic", "openai"] });
    let parsed: Value =
        serde_json::from_str(&parse_managed_plist(&input.to_string()).expect("parse"))
            .expect("json");
    assert_eq!(
        parsed,
        json!({ "share": "disabled", "server": { "hostname": "127.0.0.1", "mdns": false },
        "permission": { "*": "ask", "bash": { "*": "ask", "rm -rf *": "deny" } }, "enabled_providers": ["anthropic", "openai"] })
    );
    assert_eq!(parse_managed_plist("{}").expect("empty"), "{}");
}

#[test]
fn mdm_preferences_override_everything() {
    struct Mdm;
    impl ManagedPreferences for Mdm {
        fn read(&self, _env: &Environment) -> Option<(String, String)> {
            Some((
                "mobileconfig:/Library/Managed Preferences/ai.opencode.managed.plist".into(),
                json!({ "model": "mdm/model", "instructions": ["mdm.md"] }).to_string(),
            ))
        }
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let mut env = Environment::isolated(tmp.path().join("home"));
    env.set_var(
        "OPENCODE_CONFIG_CONTENT",
        json!({ "model": "content/model", "instructions": ["a.md"] }).to_string(),
    );
    let instance = Instance {
        directory: tmp.path().into(),
        worktree: tmp.path().into(),
    };
    let sources = Sources {
        accounts: &NoAccounts,
        remote: &UnsupportedRemote,
        managed: &Mdm,
    };
    let state = legacy::load(&env, &instance, &sources).expect("load");
    assert_eq!(state.config["model"], json!("mdm/model"));
    assert_eq!(state.config["instructions"], json!(["a.md", "mdm.md"]));
    assert_eq!(
        state.provenance.source(&["model"]),
        Some("mobileconfig:/Library/Managed Preferences/ai.opencode.managed.plist")
    );
}
