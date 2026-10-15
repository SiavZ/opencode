//! Purpose: Tauri build script for the desktop shell.
//! Scope: build-time code generation for Tauri context.
//! Uses:
//! `-- none
//! External:
//! `-- tauri_build
//! Used by:
//! `-- none, build script
//! Invariants: only runs during cargo build.

fn main() {
    tauri_build::build()
}
