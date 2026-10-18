//! Purpose: read-tool conformance suites for both generations over real temp files.
//! Scope: integration entrypoint wiring the core, legacy and shared fixture modules.
//! Uses:
//! rust/crates/
//! |-- tools/tests/files_read/core.rs
//! |-- tools/tests/files_read/filesystem.rs
//! |-- tools/tests/files_read/legacy.rs
//! `-- tools/tests/files_read/support.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real tools through `Tool::execute`, the filesystem adapter is a
//! labelled std::fs fixture until p05.native-fs lands the production adapter.
//! Contract tests: this integration suite

#[path = "files_read/core.rs"]
mod core;
#[path = "files_read/filesystem.rs"]
mod filesystem;
#[path = "files_read/legacy.rs"]
mod legacy;
#[path = "files_read/support.rs"]
mod support;
