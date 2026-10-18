//! Purpose: fixture tests for one LSP client against the fake python server: initialize params,
//! server request handlers, didOpen/didChange full + incremental (UTF-16 end), didSave/didClose,
//! push and pull diagnostics with dedupe/related docs/registrations, wait modes, init timeout,
//! joined shutdown.
//! Scope: real child process (python3 fake server), temp dirs only.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/diagnostics.rs
//! `-- integration-adapters/tests/fixtures/lsp/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only (fake server), no network, no writes outside the TempDir.
//! Contract tests: integration-adapters/tests/lsp_client.rs

#[path = "fixtures/lsp/support.rs"]
mod support;

use std::time::{Duration, Instant};

use opencode_integration_adapters::lsp::Client;
use opencode_integration_api::IntegrationError;
use opencode_integration_api::language::DiagnosticsWait;
use serde_json::json;
use support::{Workspace, fake_spec, fast_timeouts, pid_alive};

async fn start(ws: &Workspace, vars: &[(&str, &str)]) -> std::sync::Arc<Client> {
    let mut spec = fake_spec(ws, vars);
    spec.initialization = json!({"a": {"b": 7}}).as_object().cloned();
    let handle = ws.launch().spawn(&spec, &ws.dir).expect("spawn");
    Client::create("fake", handle, &ws.dir, &ws.dir, fast_timeouts())
        .await
        .expect("create")
}

#[tokio::test]
async fn initialize_handshake_and_server_requests() {
    let ws = Workspace::new();
    let client = start(&ws, &[]).await;
    ws.wait_for(|w| {
        w.methods()
            .contains(&"workspace/didChangeConfiguration".to_string())
    })
    .await;
    let init = &ws.find("initialize")[0]["params"];
    let root_uri = format!("file://{}", ws.dir.display());
    assert_eq!(init["rootUri"], root_uri);
    assert_eq!(
        init["workspaceFolders"],
        json!([{"name": "workspace", "uri": root_uri}])
    );
    assert_eq!(init["initializationOptions"], json!({"a": {"b": 7}}));
    assert_eq!(
        init["capabilities"]["textDocument"]["diagnostic"],
        json!({"dynamicRegistration": true, "relatedDocumentSupport": true})
    );
    assert_eq!(
        init["capabilities"]["textDocument"]["publishDiagnostics"],
        json!({"versionSupport": false})
    );
    assert!(init["processId"].as_u64().is_some());
    assert_eq!(
        ws.methods()[..3],
        [
            "initialize",
            "initialized",
            "workspace/didChangeConfiguration"
        ]
    );
    assert_eq!(
        ws.find("workspace/didChangeConfiguration")[0]["params"],
        json!({"settings": {"a": {"b": 7}}})
    );

    // Server -> client requests answered as baseline.
    for (method, params) in [
        (
            "workspace/configuration",
            json!({"items": [{"section": "a.b"}, {}, {"section": "missing.x"}]}),
        ),
        ("workspace/workspaceFolders", json!(null)),
        ("window/workDoneProgress/create", json!({"token": 1})),
        ("bogus/method", json!(null)),
    ] {
        client
            .connection_notify("test/trigger", json!({"method": method, "params": params}))
            .await;
    }
    ws.wait_for(|w| {
        w.messages()
            .iter()
            .filter(|m| m.get("id").is_some_and(|i| i.as_i64().unwrap_or(0) > 1000))
            .count()
            == 4
    })
    .await;
    let replies: Vec<_> = ws
        .messages()
        .into_iter()
        .filter(|m| m["id"].as_i64().unwrap_or(0) > 1000)
        .collect();
    assert_eq!(replies[0]["result"], json!([7, {"a": {"b": 7}}, null]));
    assert_eq!(
        replies[1]["result"],
        json!([{"name": "workspace", "uri": root_uri}])
    );
    assert_eq!(replies[2]["result"], json!(null));
    assert_eq!(replies[3]["error"]["code"], -32601);

    let pid = client.process_id().expect("pid");
    client.shutdown().await;
    assert!(!pid_alive(pid), "process must be reaped after shutdown");
    client.shutdown().await; // idempotent
}

#[tokio::test]
async fn document_sync_full_and_incremental_versions() {
    for (sync, incremental) in [("1", false), ("99", true)] {
        let ws = Workspace::new();
        let file = ws.write("src/a.ts", "line1\r\nx😀");
        let client = start(&ws, &[("FAKE_SYNC", sync), ("FAKE_PUSH", "0")]).await;
        assert_eq!(client.open("src/a.ts").await.unwrap(), 0);
        std::fs::write(&file, "new").unwrap();
        assert_eq!(client.open(file.to_str().unwrap()).await.unwrap(), 1);
        assert_eq!(client.open("src/a.ts").await.unwrap(), 2);
        client.save("src/a.ts").await.unwrap();
        client.close("src/a.ts").await.unwrap();
        assert!(matches!(
            client.close("src/a.ts").await,
            Err(IntegrationError::NotFound { .. })
        ));
        assert_eq!(
            client.open("src/a.ts").await.unwrap(),
            0,
            "reopen after close is a fresh didOpen"
        );
        ws.wait_for(|w| w.find("textDocument/didOpen").len() == 2)
            .await;

        let uri = format!("file://{}", file.display());
        let open = &ws.find("textDocument/didOpen")[0]["params"]["textDocument"];
        assert_eq!(
            open,
            &json!({"uri": uri, "languageId": "typescript", "version": 0, "text": "line1\r\nx😀"})
        );
        let watched: Vec<_> = ws
            .find("workspace/didChangeWatchedFiles")
            .iter()
            .map(|m| m["params"]["changes"][0]["type"].clone())
            .collect();
        assert_eq!(watched, [json!(1), json!(2), json!(2), json!(1)]);
        let change = &ws.find("textDocument/didChange")[0]["params"];
        assert_eq!(change["textDocument"], json!({"uri": uri, "version": 1}));
        let expected = if incremental {
            json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 3}}, "text": "new"}])
        } else {
            json!([{"text": "new"}])
        };
        assert_eq!(change["contentChanges"], expected);
        assert_eq!(ws.find("textDocument/didSave")[0]["params"]["text"], "new");
        assert_eq!(ws.find("textDocument/didClose").len(), 1);
        client.shutdown().await;
    }
}

#[tokio::test]
async fn push_diagnostics_wait_and_version_filter() {
    let ws = Workspace::new();
    let file = ws.write("a.ts", "x");
    let client = start(&ws, &[("FAKE_PUSH_DELAY", "0.2")]).await;
    let after = Instant::now();
    let v = client.open("a.ts").await.unwrap();
    client
        .wait_for_diagnostics("a.ts", v, DiagnosticsWait::Document, after)
        .await;
    let took = after.elapsed();
    assert!(
        took >= Duration::from_millis(200) && took < Duration::from_millis(1400),
        "push wait took {took:?}"
    );
    let diags = client.diagnostics();
    assert_eq!(diags[file.to_str().unwrap()][0].message, "push v0");

    // A wait for a version the server never publishes times out at document_wait.
    let after = Instant::now();
    client
        .wait_for_diagnostics("a.ts", 42, DiagnosticsWait::Document, after)
        .await;
    assert!(after.elapsed() >= Duration::from_millis(1400));
    client.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_dedupe_related_and_registrations() {
    let ws = Workspace::new();
    let file = ws.write("a.ts", "x");
    let client = start(
        &ws,
        &[
            ("FAKE_PULL", "1"),
            ("FAKE_PUSH", "0"),
            ("FAKE_REGISTER", "eslint"),
        ],
    )
    .await;
    ws.wait_for(|w| w.messages().iter().any(|m| m["id"] == 1001))
        .await;
    let after = Instant::now();
    let v = client.open("a.ts").await.unwrap();
    client
        .wait_for_diagnostics("a.ts", v, DiagnosticsWait::Document, after)
        .await;
    assert!(
        after.elapsed() < Duration::from_millis(1000),
        "document pull unblocks on first match"
    );
    tokio::time::sleep(Duration::from_millis(200)).await; // background identifier pull merges
    let diags = client.diagnostics();
    let mine: Vec<_> = diags[file.to_str().unwrap()]
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(mine.contains(&"pull static".to_string()), "{mine:?}");
    assert_eq!(
        mine.iter().filter(|m| *m == "pull static").count(),
        1,
        "deduped"
    );
    assert_eq!(diags["/related/other.ts"][0].message, "related");
    let idents: Vec<_> = ws
        .find("textDocument/diagnostic")
        .iter()
        .map(|m| m["params"].get("identifier").cloned())
        .collect();
    assert!(
        idents.contains(&None) && idents.contains(&Some(json!("eslint"))),
        "{idents:?}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn full_mode_uses_workspace_diagnostics() {
    let ws = Workspace::new();
    ws.write("a.ts", "x");
    let client = start(&ws, &[("FAKE_PUSH", "0"), ("FAKE_WORKSPACE_REGISTER", "1")]).await;
    ws.wait_for(|w| w.messages().iter().any(|m| m["id"] == 1001))
        .await;
    let after = Instant::now();
    let v = client.open("a.ts").await.unwrap();
    client
        .wait_for_diagnostics("a.ts", v, DiagnosticsWait::Full, after)
        .await;
    assert!(after.elapsed() < Duration::from_millis(1500));
    let req = &ws.find("workspace/diagnostic");
    assert_eq!(req.len(), 2, "unnamed + identifier 'ws'");
    assert_eq!(req[0]["params"]["previousResultIds"], json!([]));
    assert_eq!(client.diagnostics()["/ws/a.ts"][0].message, "workspace");
    client.shutdown().await;
}

#[tokio::test]
async fn initialize_timeout_kills_process() {
    let ws = Workspace::new();
    let spec = fake_spec(&ws, &[("FAKE_HANG_INIT", "1")]);
    let handle = ws.launch().spawn(&spec, &ws.dir).unwrap();
    let pid = handle.child.id().unwrap();
    let mut t = fast_timeouts();
    t.initialize = Duration::from_millis(300);
    let err = Client::create("fake", handle, &ws.dir, &ws.dir, t)
        .await
        .err()
        .expect("must fail");
    assert!(
        matches!(&err, IntegrationError::Failed { message, .. } if message.contains("LSPInitializeError")),
        "{err:?}"
    );
    assert!(!pid_alive(pid));
}
