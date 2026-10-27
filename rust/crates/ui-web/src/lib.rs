//! Purpose: crate root of the browser application. Composes the renderer and injects the host.
//! Scope: wasm32 entrypoint (`start`) plus a native-checkable component tree.
//! Uses:
//! rust/crates/
//! |-- ui-web/src/commands.rs
//! |-- ui-web/src/composer/mod.rs
//! |-- ui-web/src/design/mod.rs
//! |-- ui-web/src/host/mod.rs
//! |-- ui-web/src/locale.rs
//! |-- ui-web/src/markdown/mod.rs
//! |-- ui-web/src/panels/mod.rs
//! |-- ui-web/src/routes.rs
//! |-- ui-web/src/session.rs
//! |-- ui-web/src/settings.rs
//! `-- ui-web/src/timeline/mod.rs
//! External:
//! |-- console_error_panic_hook::set_once
//! |-- leptos::mount::mount_to_body
//! |-- leptos::prelude::*
//! `-- wasm_bindgen::prelude::wasm_bindgen
//! Used by:
//! `-- none, entrypoint
//! Invariants: never depends on server, runtime, session-runtime, storage-sqlite or platform-native.
//! Child ownership: shell (lib, routes, commands, settings, host), session (session, timeline),
//! composer (composer, panels), design (design, locale), render (markdown).
//! Contract tests: ui-web/tests

pub mod commands;
pub mod composer;
pub mod design;
pub mod host;
pub mod locale;
pub mod markdown;
pub mod panels;
pub mod routes;
pub mod session;
pub mod settings;
pub mod state;
pub mod timeline;

use leptos::prelude::*;
use wasm_bindgen::prelude::wasm_bindgen;

#[component]
pub fn App() -> impl IntoView {
    view! { <main class="oc-app"><routes::AppRoutes/></main> }
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
