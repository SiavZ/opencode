//! Purpose: Tauri 2 desktop shell entrypoint. Hosts the ui-web bundle, wires native
//! windows/menus, single-instance arbitration, deep links, and the desktop-host bridge.
//! Scope: process entrypoint. Only binary that depends on tauri.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! External:
//! |-- anyhow
//! |-- tauri
//! |-- tauri_plugin_deep_link
//! `-- tauri_plugin_single_instance
//! Used by:
//! `-- none, entrypoint
//! Invariants: tauri is only a dependency of this binary, never of wasm-compiled crates.
//! Contract tests: none yet, smoke runtime option provides basic validation.

use std::env;
use std::time::Duration;

use anyhow::Result;
use tauri::Manager;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let smoke_test = env::args().any(|arg| arg == "--smoke-test");

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            tracing::info!("Second instance launched with argv: {:?}", argv);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(move |app| {
            tracing::info!("window-created");

            let _window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("OpenCode Desktop")
            .build()?;

            if smoke_test {
                let handle = app.handle().clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    tracing::info!("smoke-test exiting gracefully");
                    handle.exit(0);
                });
            }

            Ok(())
        });

    builder
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri application error: {}", e))
}
