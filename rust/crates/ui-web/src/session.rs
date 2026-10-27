//! Purpose: render a session workspace and actionable failures around the canonical timeline.
//! Scope: browser session view.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: failures stay visible and retry uses the real transport.
//! Contract tests: browser acceptance
use crate::state::{Browser, text};
use leptos::prelude::*;
#[component]
pub fn SessionPage() -> impl IntoView {
    let browser = expect_context::<Browser>();
    view! {
        <section class="oc-session" aria-label="Session workspace">
            <header class="oc-session-header"><h1>{move ||browser.state.with(|state|state.sessions.iter().find(|session|session["id"]==state.active).map(|session|text(session,"title")).unwrap_or_else(||"New session".into()))}</h1><span role="status">{move ||browser.state.with(|state|if state.loading { "Loading" } else { "" })}</span></header>
            <Show when=move ||browser.state.with(|state|state.error.is_some())>
                <div class="oc-error" role="alert">{move ||browser.state.with(|state|state.error.clone().unwrap_or_default())}<button on:click=move |_| { browser.state.update(|state|state.error=None); browser.refresh(); if browser.state.with(|state|!state.active.is_empty()) { browser.load(None); } }>"Retry"</button></div>
            </Show>
            <crate::timeline::Timeline/>
            <crate::composer::Composer/>
        </section>
    }
}
