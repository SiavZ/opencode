//! Purpose: session page composition: timeline, composer, side panels for one scoped session.
//! Scope: session view.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! `-- leptos::prelude
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.session; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.session)

use leptos::prelude::*;

#[component]
pub fn SessionPage() -> impl IntoView {
    view! { <div class="oc-session"><crate::composer::Composer/></div> }
}
