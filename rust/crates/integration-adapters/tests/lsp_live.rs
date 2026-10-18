//! Purpose: LIVE tests against real language servers installed on the host (rust-analyzer, clangd)
//! through the builtin inventory and `LanguageService`. Skipped (pass with a note) when the server
//! is absent from PATH.
//! Scope: real processes, temp workspace, downloads disabled so nothing is installed.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/fixtures/lsp/support.rs
//! External:
//! `-- which::which
//! Used by:
//! `-- none, entrypoint
//! Invariants: live-labelled, never runs an installer (disable_download = true).
//! Contract tests: integration-adapters/tests/lsp_live.rs

#[path = "fixtures/lsp/support.rs"]
mod support;

use std::time::Duration;

use opencode_integration_adapters::lsp::{LspOptions, LspService, Timeouts};
use opencode_integration_api::language::{DiagnosticsWait, LanguageService, LocInput, LspConfig};
use support::Workspace;

fn live_service(ws: &Workspace) -> LspService {
    let mut launch = ws.launch();
    launch.path = std::env::var_os("PATH");
    LspService::new(LspOptions {
        config: Some(LspConfig::Enabled(true)),
        context: ws.ctx(),
        launch,
        timeouts: Timeouts {
            request: Some(Duration::from_secs(60)),
            ..Timeouts::default()
        },
    })
}

#[tokio::test]
async fn live_clangd_diagnostics_and_hover() {
    if which::which("clangd").is_err() {
        eprintln!("LIVE SKIP: clangd not on PATH");
        return;
    }
    let ws = Workspace::new();
    ws.write("compile_flags.txt", "-std=c11\n");
    let file = ws.write(
        "main.c",
        "int add(int a, int b) { return a + b; }\nint main(void) { return add(1, missing); }\n",
    );
    let svc = live_service(&ws);
    svc.touch_file("main.c", Some(DiagnosticsWait::Document))
        .await
        .unwrap();
    let status = svc.status().await;
    assert!(status.iter().any(|s| s.id == "clangd"), "{status:?}");
    let diags = svc.diagnostics().await;
    let mine = diags
        .get(file.to_str().unwrap())
        .cloned()
        .unwrap_or_default();
    assert!(
        mine.iter().any(|d| d.message.contains("missing")),
        "clangd diagnostics: {mine:?}"
    );
    let hover = svc
        .hover(LocInput {
            file: "main.c".into(),
            line: 1,
            character: 25,
        })
        .await
        .unwrap();
    assert!(hover.to_string().contains("add"), "hover: {hover}");
    let def = svc
        .definition(LocInput {
            file: "main.c".into(),
            line: 1,
            character: 25,
        })
        .await
        .unwrap();
    assert_eq!(def[0]["range"]["start"]["line"], 0);
    svc.shutdown().await;
    assert!(svc.status().await.is_empty());
}

#[tokio::test]
async fn live_rust_analyzer_symbols() {
    let works = std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !works || std::env::var_os("OPENCODE_LSP_LIVE_RA").is_none() {
        eprintln!(
            "LIVE SKIP: needs a working rust-analyzer (not a bare rustup proxy) and OPENCODE_LSP_LIVE_RA=1"
        );
        return;
    }
    let ws = Workspace::new();
    ws.write(
        "Cargo.toml",
        "[package]\nname = \"live\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let file = ws.write("src/lib.rs", "pub fn answer() -> u32 { 42 }\n");
    let svc = live_service(&ws);
    svc.touch_file("src/lib.rs", None).await.unwrap();
    assert!(svc.status().await.iter().any(|s| s.id == "rust"));
    let uri = format!("file://{}", file.display());
    let mut names = Vec::new();
    for _ in 0..30 {
        names = svc
            .document_symbol(&uri)
            .await
            .unwrap()
            .into_iter()
            .map(|s| format!("{s:?}"))
            .collect();
        if names.iter().any(|n| n.contains("answer")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(names.iter().any(|n| n.contains("answer")), "{names:?}");
    svc.shutdown().await;
}
