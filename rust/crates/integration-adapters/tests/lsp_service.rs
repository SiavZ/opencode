//! Purpose: fixture tests for `LspService` (LanguageService impl) against the fake python server
//! configured as a custom `lsp.<id>` entry: (server, root) dedup under concurrency, broken set and
//! restart, location guard, status, every feature request shape, merged diagnostics, updated
//! events and joined shutdown.
//! Scope: real child processes (python3 fake server), temp dirs, empty PATH for builtins.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/fixtures/lsp/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only, builtins resolve against an empty PATH with downloads disabled so no
//! real server or installer can run.
//! Contract tests: integration-adapters/tests/lsp_service.rs

#[path = "fixtures/lsp/support.rs"]
mod support;

use std::sync::Arc;

use opencode_integration_adapters::lsp::{LspOptions, LspService};
use opencode_integration_api::language::{
    AnySymbol, DiagnosticsWait, LanguageService, LocInput, LspConfig,
};
use serde_json::json;
use support::{Workspace, fake_script, fast_timeouts, pid_alive};

fn python() -> String {
    which::which("python3")
        .expect("python3 on PATH")
        .to_string_lossy()
        .into_owned()
}

fn service(ws: &Workspace, command: Vec<String>) -> Arc<LspService> {
    let empty = ws.tmp.path().join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let mut launch = ws.launch();
    launch.path = Some(empty.into_os_string());
    let config: LspConfig = serde_json::from_value(json!({
        "fake": {"extensions": [".ts"], "command": command, "env": {"FAKE_LSP_LOG": ws.log}},
        "eslint": {"disabled": true}
    }))
    .unwrap();
    Arc::new(LspService::new(LspOptions {
        config: Some(config),
        context: ws.ctx(),
        launch,
        timeouts: fast_timeouts(),
    }))
}

fn fake_command() -> Vec<String> {
    vec![
        python(),
        "-u".into(),
        fake_script().to_string_lossy().into_owned(),
    ]
}

fn loc(file: &str) -> LocInput {
    LocInput {
        file: file.into(),
        line: 3,
        character: 4,
    }
}

#[tokio::test]
async fn dedup_broken_status_and_shutdown() {
    let ws = Workspace::new();
    ws.write("a.ts", "a");
    ws.write("sub/b.ts", "b");
    let svc = service(&ws, fake_command());
    assert!(svc.server_ids().contains(&"fake".to_string()));
    assert!(
        !svc.server_ids().contains(&"eslint".to_string()),
        "disabled entry removed"
    );
    let mut updated = svc.subscribe_updated();

    // Concurrent touches of two files in the same root share one spawn.
    let (a, b) = tokio::join!(
        svc.touch_file("a.ts", None),
        svc.touch_file("sub/b.ts", None)
    );
    a.unwrap();
    b.unwrap();
    let starts = ws
        .messages()
        .iter()
        .filter(|m| m["event"] == "start")
        .count();
    assert_eq!(starts, 1, "one process per (server, root)");
    assert!(
        updated.try_recv().is_ok() && updated.try_recv().is_err(),
        "one lsp.updated"
    );

    let status = svc.status().await;
    assert_eq!(status.len(), 1);
    assert_eq!(
        (status[0].id.as_str(), status[0].root.as_str()),
        ("fake", "")
    );

    // Builtins for .ts are unavailable (empty PATH, downloads disabled) and land in the broken set.
    let broken = svc.broken();
    assert!(
        broken.iter().any(|k| k.ends_with("typescript")),
        "{broken:?}"
    );
    assert!(svc.has_clients("a.ts").await, "fake still serves .ts");
    // Baseline hasClients only checks extension, root and broken set, not availability.
    assert!(
        svc.has_clients("x.py").await,
        "pyright matches until it is tried and broken"
    );
    assert!(!svc.has_clients("x.unknownext").await);

    // Files outside the Location get no clients.
    svc.touch_file("/definitely/elsewhere/c.ts", None)
        .await
        .unwrap();
    assert_eq!(
        ws.messages()
            .iter()
            .filter(|m| m["event"] == "start")
            .count(),
        1
    );

    let pid = ws
        .messages()
        .iter()
        .find(|m| m["event"] == "start")
        .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    svc.shutdown().await;
    assert!(!pid_alive(pid));
    assert!(svc.status().await.is_empty());
    svc.touch_file("a.ts", None).await.unwrap();
    assert_eq!(
        ws.messages()
            .iter()
            .filter(|m| m["event"] == "start")
            .count(),
        1,
        "closed service never respawns"
    );
}

#[tokio::test]
async fn broken_custom_server_and_restart() {
    let ws = Workspace::new();
    ws.write("a.ts", "a");
    let svc = service(
        &ws,
        vec![ws.dir.join("missing-binary").to_string_lossy().into_owned()],
    );
    svc.touch_file("a.ts", None).await.unwrap();
    let key = format!("{}fake", ws.dir.display());
    assert!(svc.broken().contains(&key));
    assert!(svc.status().await.is_empty());
    svc.restart().await;
    assert!(svc.broken().is_empty(), "restart clears the broken set");
}

#[tokio::test]
async fn feature_requests_match_baseline_shapes() {
    let ws = Workspace::new();
    let file = ws.write("a.ts", "a");
    let svc = service(&ws, fake_command());
    svc.touch_file("a.ts", Some(DiagnosticsWait::Document))
        .await
        .unwrap();
    let uri = format!("file://{}", file.display());

    let hover = svc.hover(loc("a.ts")).await.unwrap();
    assert_eq!(hover, json!([{"contents": "hover 3:4"}]));
    let def = svc.definition(loc("a.ts")).await.unwrap();
    assert_eq!(def.len(), 1);
    assert_eq!(def[0]["uri"], uri);
    assert_eq!(
        svc.references(loc("a.ts")).await.unwrap().len(),
        2,
        "includeDeclaration sent"
    );
    assert!(
        svc.implementation(loc("a.ts")).await.unwrap().is_empty(),
        "null filtered"
    );
    let symbols = svc.document_symbol(&uri).await.unwrap();
    assert!(matches!(&symbols[0], AnySymbol::Document(d) if d.name == "main"));
    let ws_symbols = svc.workspace_symbol("s").await.unwrap();
    assert_eq!(ws_symbols.len(), 10, "kind filter then cap 10");
    assert!(ws_symbols.iter().all(|s| s.kind != 1 && s.kind != 2));
    assert_eq!(
        svc.prepare_call_hierarchy(loc("a.ts")).await.unwrap()[0]["name"],
        "f"
    );
    let incoming = svc.incoming_calls(loc("a.ts")).await.unwrap();
    assert_eq!(incoming[0]["direction"], "callHierarchy/incomingCalls");
    assert_eq!(incoming[0]["item"]["name"], "f");
    let outgoing = svc.outgoing_calls(loc("a.ts")).await.unwrap();
    assert_eq!(outgoing[0]["direction"], "callHierarchy/outgoingCalls");

    let diags = svc.diagnostics().await;
    assert_eq!(diags[file.to_str().unwrap()][0].message, "push v0");
    let refs = ws.find("textDocument/references");
    assert_eq!(
        refs[0]["params"]["context"],
        json!({"includeDeclaration": true})
    );
    assert_eq!(
        refs[0]["params"]["position"],
        json!({"line": 3, "character": 4})
    );
    svc.shutdown().await;
}

#[tokio::test]
async fn disabled_config_has_no_servers() {
    let ws = Workspace::new();
    for config in [None, Some(LspConfig::Enabled(false))] {
        let svc = LspService::new(LspOptions {
            config,
            context: ws.ctx(),
            launch: ws.launch(),
            timeouts: fast_timeouts(),
        });
        assert!(svc.server_ids().is_empty());
        assert!(!svc.has_clients("a.ts").await);
    }
}
