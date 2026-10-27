//! Purpose: navigate canonical server/session routes without recreating server-owned cache.
//! Scope: browser shell, history, directory and server selection.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: server changes reset server-owned data; session changes retain it.
//! Contract tests: ui-web/tests/routes.rs and browser acceptance
use crate::state::{Browser, BrowserState, text};
use leptos::prelude::*;
use opencode_view_model::identity::{decode_segment, require_server_key};
use wasm_bindgen::{JsCast, closure::Closure};

pub fn parse(path: &str, origin: &str) -> Result<(String, String, Option<String>), String> {
    let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["server", server, "session", id] if !id.is_empty() => Ok((
            require_server_key(server)
                .map_err(|_| "Invalid server route")?
                .0,
            (*id).into(),
            None,
        )),
        [directory, "session", id] => Ok((
            origin.into(),
            (*id).into(),
            Some(decode_segment(directory).ok_or("Invalid directory route")?),
        )),
        [""] | ["new-session"] => Ok((origin.into(), String::new(), None)),
        _ => Err("This route is not supported.".into()),
    }
}

fn follow(browser: Browser) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let route = location
        .pathname()
        .map_err(|_| "Cannot read browser location".into())
        .and_then(|path| parse(&path, &location.origin().unwrap_or_default()));
    match route {
        Ok((server, id, directory)) => {
            let changed = browser.state.with(|state| state.server != server);
            if changed {
                browser.stream.update_value(|stream| *stream = None);
                browser.state.set(BrowserState::new(server));
            }
            if let Some(directory) = directory {
                browser.state.update(|state| state.directory = directory);
            }
            browser.select(id, false);
            browser.refresh();
        }
        Err(error) => browser.error(error),
    }
}

#[component]
pub fn AppRoutes() -> impl IntoView {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_else(|| "http://localhost".into());
    let browser = Browser {
        state: RwSignal::new(BrowserState::new(origin)),
        stream: StoredValue::new_local(None),
    };
    provide_context(browser);
    let pop = Closure::new(move |_: web_sys::Event| follow(browser));
    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback("popstate", pop.as_ref().unchecked_ref());
    }
    let listener = StoredValue::new_local(pop);
    on_cleanup(move || {
        if let Some(window) = web_sys::window() {
            listener.with_value(|listener| {
                let _ = window.remove_event_listener_with_callback(
                    "popstate",
                    listener.as_ref().unchecked_ref(),
                );
            });
        }
        browser.stream.update_value(|stream| *stream = None);
    });
    follow(browser);
    view! {
        <style>{include_str!("design/app.css")}</style>
        <aside class="oc-sidebar" aria-label="Sessions">
            <header><strong>"opencode"</strong><button on:click=move |_|browser.select(String::new(),true)>"New session"</button></header>
            <label>"Server"<input aria-label="Server URL" prop:value=move ||browser.state.with(|state|state.server.clone()) on:change=move |event| {
                let server=event_target_value(&event);
                match url::Url::parse(&server) {
                    Ok(url) if matches!(url.scheme(),"http"|"https") => { browser.stream.update_value(|stream|*stream=None); browser.state.set(BrowserState::new(server)); browser.select(String::new(),true); browser.refresh(); }
                    _ => browser.error("Use an http or https server URL.".into()),
                }
            }/></label>
            <label>"Directory"<input aria-label="Project directory" prop:value=move ||browser.state.with(|state|state.directory.clone()) on:change=move |event| {
                let directory=event_target_value(&event);
                crate::host::storage::set("oc.directory",&directory);
                browser.state.update(|state|state.directory=directory);
                browser.select(String::new(),true); browser.refresh();
            }/></label>
            <button on:click=move |_|browser.refresh()>"Refresh sessions"</button>
            <nav><For each=move ||browser.state.with(|state|state.sessions.clone()) key=|session|text(session,"id") children=move |session| {
                let id=text(&session,"id"); let active=id.clone();
                view! { <button class:active=move ||browser.state.with(|state|state.active==active) on:click=move |_|browser.select(id.clone(),true)>{text(&session,"title")}</button> }
            }/></nav>
        </aside>
        <crate::session::SessionPage/>
    }
}
