//! Purpose: route table, server-key root ownership, legacy redirects and history integration.
//! Scope: navigation.
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

use leptos::prelude::*;

#[component]
pub fn AppRoutes() -> impl IntoView {
    view! { <crate::session::SessionPage/> }
}
