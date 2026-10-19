//! Purpose: portable file-tool services and generation-specific baseline contracts.
//! Scope: file-tool module exports.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/jsdiff.rs
//! |-- tools/src/files/mutation_input.rs
//! |-- tools/src/files/patch.rs
//! |-- tools/src/files/read_common.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_files.rs
//! |-- tools/src/files/read_input.rs
//! |-- tools/src/files/read_legacy.rs
//! |-- tools/src/files/read_lines.rs
//! |-- tools/src/files/read_pager.rs
//! |-- tools/src/files/replacers.rs
//! |-- tools/src/files/schemas.rs
//! `-- tools/src/files/write.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- runtime/src/location_tools.rs
//! |-- tools/src/files/patch.rs
//! |-- tools/src/files/patch_derive.rs
//! |-- tools/src/lib.rs
//! |-- tools/tests/files_contract.rs
//! |-- tools/tests/files_mutation/native.rs
//! |-- tools/tests/files_mutation/support.rs
//! |-- tools/tests/files_patch.rs
//! |-- tools/tests/files_patch/content.rs
//! |-- tools/tests/files_patch/live.rs
//! `-- tools/tests/files_read/support.rs
//! Invariants: native adapters enter only through platform-api traits.
//! Contract tests: tools/tests/files_common.rs, tools/tests/files_contract.rs

pub mod common;
pub mod edit;
pub mod edit_legacy;
pub mod jsdiff;
pub mod mutation_input;
pub mod patch;
pub mod read_common;
pub mod read_core;
pub mod read_files;
pub mod read_input;
pub mod read_legacy;
pub mod read_lines;
mod read_pager;
pub mod replacers;
pub mod schemas;
pub mod write;
pub use common::{FileServices, Generation};
