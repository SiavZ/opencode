//! Purpose: crate root for built-in leaf tools and tool output bounding.
//! Scope: module declarations only. Each family is owned by one Phase 05 child node.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! |-- tools/src/interaction/mod.rs
//! |-- tools/src/search/mod.rs
//! |-- tools/src/shell/mod.rs
//! `-- tools/src/web/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: leaves authorize before reading target content or causing side effects; tool
//! descriptions are byte-identical to the baseline `.txt` files.
//! Contract tests: tools/tests/

/// read, write, edit (all replacers), multiedit, apply_patch, list, path resolution (p05.tools-files).
pub mod files;
/// Built-in assembly/gating, todo, question, skill, task, plan, batch, lsp, invalid, and output
/// truncation/retention (p05.tools-interaction).
pub mod interaction;
/// glob, grep via ripgrep, ripgrep binary resolution (p05.tools-exec).
pub mod search;
/// bash/shell tool with timeout/abort/output bounds and background jobs (p05.tools-exec).
pub mod shell;
/// webfetch and websearch (p05.tools-interaction).
pub mod web;
