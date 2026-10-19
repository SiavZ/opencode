//! Purpose: side panels: file tree, diff viewer, terminal panel.
//! Scope: panels.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.composer; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.composer)

pub const PANEL_IDS: [&str; 3] = ["files", "review", "terminal"];
