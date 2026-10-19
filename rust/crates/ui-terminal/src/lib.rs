//! Purpose: terminal UI crate root. Declares the renderer lifecycle, input, theme, prompt, timeline, routes, dialogs and plugin host modules.
//! Scope: crate root.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/app.rs
//! |-- ui-terminal/src/dialog/mod.rs
//! |-- ui-terminal/src/input.rs
//! |-- ui-terminal/src/keybind.rs
//! |-- ui-terminal/src/plugin_host.rs
//! |-- ui-terminal/src/prompt/mod.rs
//! |-- ui-terminal/src/routes/mod.rs
//! |-- ui-terminal/src/terminal.rs
//! |-- ui-terminal/src/theme/mod.rs
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: library never writes to the real terminal except through terminal.rs guards.
//! Contract tests: ui-terminal/tests/
pub mod app;
pub mod dialog;
pub mod input;
pub mod keybind;
pub mod plugin_host;
pub mod prompt;
pub mod rendering;
pub mod routes;
pub mod terminal;
pub mod theme;
pub mod timeline;
