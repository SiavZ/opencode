//! Purpose: browser host bindings: localStorage drafts/settings, clipboard, history, server connection (fetch + SSE) feeding the view-model reducer.
//! Scope: wasm32 browser APIs behind small typed functions.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! `-- leptos::prelude
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.shell; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.shell)

pub mod storage;
