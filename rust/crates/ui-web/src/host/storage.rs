//! Purpose: typed localStorage access for drafts, settings and layout persistence.
//! Scope: browser storage.
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

/// Read a persisted value. Returns `None` outside a browser or when absent.
pub fn get(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?
}

/// Persist a value. Silently ignored when storage is unavailable (private mode).
pub fn set(key: &str, value: &str) {
    if let Some(Ok(Some(s))) = web_sys::window().map(|w| w.local_storage()) {
        let _ = s.set_item(key, value);
    }
}
