//! Purpose: crate root for the native desktop host. Owns every operation the Electron
//! main/preload processes performed, exposed as typed capabilities. Does not depend on tauri,
//! the shell binary (`bins/desktop`) adapts these capabilities to the webview.
//! Scope: native only, never compiled to wasm.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/attachments.rs
//! |-- desktop-host/src/bridge.rs
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/deeplinks.rs
//! |-- desktop-host/src/drafts.rs
//! |-- desktop-host/src/lifecycle.rs
//! |-- desktop-host/src/menu.rs
//! |-- desktop-host/src/migrate.rs
//! |-- desktop-host/src/platform/mod.rs
//! |-- desktop-host/src/store.rs
//! |-- desktop-host/src/updater.rs
//! `-- desktop-host/src/windows.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- desktop/src/main.rs
//! Invariants: no tauri dependency, module ownership is per child node (see contracts/phase-11/contract.md).
//! Contract tests: desktop-host/tests/

pub mod attachments;
pub mod bridge;
pub mod connections;
pub mod deeplinks;
pub mod drafts;
pub mod lifecycle;
pub mod menu;
pub mod migrate;
pub mod platform;
pub mod store;
pub mod updater;
pub mod windows;
