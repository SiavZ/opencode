//! Purpose: prompt composer: editing, IME composition, drafts, attachments, submit with captured target.
//! Scope: composer.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! `-- leptos::prelude
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.composer; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.composer)

use leptos::prelude::*;

#[component]
pub fn Composer() -> impl IntoView {
    view! { <textarea class="oc-composer" aria-label="Prompt"></textarea> }
}
