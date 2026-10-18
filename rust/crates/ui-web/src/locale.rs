//! Purpose: locale dictionaries from packages/app and packages/ui i18n, fallback, plurals, RTL.
//! Scope: localization.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! `-- leptos::prelude
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.design; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.design)

/// Translate a key in the active locale with English fallback; returns the key when unknown.
pub fn t(key: &str) -> String {
    key.to_owned()
}
