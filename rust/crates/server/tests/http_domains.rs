//! Purpose: live loopback tests for newly mounted legacy families, raw payloads and query bounds.
//! Scope: real HTTP listener with absent domain services; no service success is fabricated.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/cors.rs
//! `-- server/src/services/mod.rs
//! External:
//! |-- reqwest::Client
//! |-- serde_json::Value
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: valid requests reach their named domain port; invalid requests fail before service dispatch.
//! Contract tests: this file; actual composed service parity is tested by runtime/tests/strict_replay.rs.

use opencode_server::app::{ServerConfig, router};
use opencode_server::lifecycle::{ListenOptions, Listener, listen};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;
use opencode_server::services::Services;
use serde_json::{Value, json};

async fn start() -> Listener {
    listen(
        router(
            Services::default(),
            ServerConfig::full_host(AuthConfig::default(), CorsConfig::default()),
        ),
        ListenOptions {
            hostname: "127.0.0.1".into(),
            port: 0,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn legacy_families_reach_explicit_ports_without_ui_fallthrough() {
    let server = start().await;
    let client = reqwest::Client::new();
    for (path, operation) in [
        ("/config", "config.get"),
        ("/config/providers", "config.providers"),
        ("/project", "project.list"),
        ("/project/current", "project.current"),
        ("/project/global/directories", "project.directories"),
        ("/file?path=.", "file.list"),
        ("/file/content?path=missing", "file.read"),
        ("/file/status", "file.status"),
        ("/find?pattern=test", "find.text"),
        ("/find/file?query=test&limit=200", "find.files"),
        ("/find/symbol?query=test", "find.symbols"),
        ("/path", "path.get"),
        ("/vcs", "vcs.get"),
        ("/vcs/status", "vcs.status"),
        ("/vcs/diff?mode=git&context=0", "vcs.diff"),
        ("/vcs/diff/raw", "vcs.diff.raw"),
        ("/command", "command.list"),
        ("/agent", "app.agents"),
        ("/skill", "app.skills"),
        ("/lsp", "lsp.status"),
        ("/formatter", "formatter.status"),
        ("/mcp", "mcp.status"),
        ("/provider", "provider.list"),
        ("/provider/auth", "provider.auth"),
        ("/permission", "permission.list"),
        ("/question", "question.list"),
        ("/pty", "pty.list"),
        ("/pty/shells", "pty.shells"),
        ("/session/ses_missing/children", "session.children"),
        ("/session/ses_missing/todo", "session.todo"),
        ("/session/ses_missing/diff", "session.diff"),
        (
            "/experimental/capabilities",
            "experimental.capabilities.get",
        ),
        (
            "/experimental/session?roots=true&archived=false",
            "experimental.session.list",
        ),
        ("/experimental/worktree", "worktree.list"),
        ("/experimental/resource", "experimental.resource.list"),
        ("/experimental/workspace", "experimental.workspace.list"),
        (
            "/experimental/workspace/adapter",
            "experimental.workspace.adapter.list",
        ),
        (
            "/experimental/workspace/status",
            "experimental.workspace.status",
        ),
        ("/experimental/tool/ids", "tool.ids"),
        ("/experimental/tool?provider=local&model=test", "tool.list"),
        ("/global/config", "global.config.get"),
        ("/tui/control/next", "tui.control.next"),
    ] {
        let response = client
            .get(format!("{}{path}", server.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["service"],
            operation,
            "{path}"
        );
    }
    server.stop(false).await;
}

#[tokio::test]
async fn legacy_required_queries_and_numeric_bounds_reject_before_services() {
    let server = start().await;
    let client = reqwest::Client::new();
    for path in [
        "/file",
        "/file/content",
        "/find",
        "/find/file",
        "/find/symbol",
        "/find/file?query=x&limit=0",
        "/find/file?query=x&limit=201",
        "/find/file?query=x&limit=1.5",
        "/find/file?query=x&type=other",
        "/find/file?query=x&dirs=yes",
        "/vcs/diff",
        "/vcs/diff?mode=other",
        "/vcs/diff?mode=git&context=-1",
        "/experimental/tool?provider=test",
        "/experimental/session?roots=maybe",
    ] {
        let response = client
            .get(format!("{}{path}", server.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
        let error = response.json::<Value>().await.unwrap();
        assert_eq!(error["name"], "BadRequest", "{path}");
        assert_eq!(error["data"]["kind"], "Query", "{path}");
    }
    server.stop(false).await;
}

#[tokio::test]
async fn fork_and_provider_authorization_keep_distinct_raw_body_contracts() {
    let server = start().await;
    let client = reqwest::Client::new();
    let fork = format!("{}/session/ses_missing/fork", server.url());
    for body in ["", " \n\t", "{}", "{\"messageID\":\"msg_before\"}"] {
        let response = client.post(&fork).body(body).send().await.unwrap();
        assert_eq!(response.status(), 503, "{body:?}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["service"],
            "session.fork"
        );
    }
    for body in ["{", "null", "[]", "{\"messageID\":42}"] {
        let response = client.post(&fork).body(body).send().await.unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"_tag":"BadRequest"})
        );
    }
    let authorize = format!("{}/provider/local/oauth/authorize", server.url());
    for body in ["", "{", "null", "{}", "{\"method\":\"wrong\"}"] {
        let response = client.post(&authorize).body(body).send().await.unwrap();
        assert_eq!(response.status(), 400, "{body:?}");
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"name":"BadRequest","data":{}})
        );
    }
    let response = client
        .post(&authorize)
        .body("{\"method\":0}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.json::<Value>().await.unwrap()["service"],
        "provider.oauth.authorize"
    );
    server.stop(false).await;
}

#[tokio::test]
async fn named_payloads_and_empty_bodies_preserve_baseline_errors() {
    let server = start().await;
    let client = reqwest::Client::new();
    for (method, path, name) in [
        (reqwest::Method::PUT, "/auth/local", "Auth"),
        (reqwest::Method::PATCH, "/config", "Config"),
        (reqwest::Method::PATCH, "/global/config", "Config"),
        (
            reqwest::Method::DELETE,
            "/experimental/worktree",
            "WorktreeRemoveInput",
        ),
    ] {
        let response = client
            .request(method, format!("{}{path}", server.url()))
            .json(&json!([1, 2, 3]))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({
                "name":"BadRequest",
                "data":{"message":format!("Expected {name}, got [1,2,3]"),"kind":"Payload"}
            }),
            "{path}"
        );
    }
    let response = client
        .post(format!("{}/session/ses_missing/shell", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({
            "name":"BadRequest",
            "data":{"message":"Expected object, got undefined","kind":"Payload"}
        })
    );
    let response = client
        .post(format!("{}/sync/replay", server.url()))
        .json(&json!({"directory":"/fixture","events":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["message"],
        "Missing key\n  at [\"events\"][0]"
    );
    server.stop(false).await;
}

#[tokio::test]
async fn native_payloads_reject_invalid_enums_and_nested_shapes() {
    let server = start().await;
    let client = reqwest::Client::new();
    for (path, payload) in [
        ("/permission/request/reply", json!({"reply":"allow"})),
        (
            "/question/request/reply",
            json!({"answers":["not-an-array"]}),
        ),
        ("/pty", json!({"args":"not-an-array"})),
        ("/mcp", json!({"name":"local","config":{"type":"local"}})),
        ("/session/ses_missing/revert", json!({})),
        (
            "/session/ses_missing/message",
            json!({"parts":[{"type":"text"}]}),
        ),
        ("/sync/replay", json!({"directory":"/fixture","events":[]})),
    ] {
        let response = client
            .post(format!("{}{path}", server.url()))
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}: {payload}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["data"]["kind"],
            "Payload"
        );
    }
    server.stop(false).await;
}
