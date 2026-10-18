//! Purpose: write-tool conformance suites for both generations over real temp files.
//! Scope: integration entrypoint wiring the write module and shared mutation fixtures.
//! Uses:
//! rust/crates/
//! |-- tools/tests/files_mutation/native.rs
//! |-- tools/tests/files_mutation/support.rs
//! `-- tools/tests/files_mutation/write.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real tools through `Tool::execute`, over labelled dev adapters
//! (read/write counting) and, in `native`, over the production platform-native adapters.
//! Contract tests: this integration suite

#[path = "files_mutation/native.rs"]
mod native;
#[path = "files_mutation/support.rs"]
mod support;
#[path = "files_mutation/write.rs"]
mod write;
