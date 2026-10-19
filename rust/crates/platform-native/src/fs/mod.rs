//! Purpose: native filesystem adapter family: raw I/O, serialized mutations, canonical Location
//! resolution, encoding detection, ignore/protected rules, glob scans and the notify watcher.
//! Scope: native adapter module exports.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/fs/encoding.rs
//! |-- platform-native/src/fs/error.rs
//! |-- platform-native/src/fs/glob.rs
//! |-- platform-native/src/fs/ignore.rs
//! |-- platform-native/src/fs/location.rs
//! |-- platform-native/src/fs/mutation.rs
//! |-- platform-native/src/fs/native.rs
//! `-- platform-native/src/fs/watcher.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- platform-native/src/lib.rs
//! |-- platform-native/tests/fs_location.rs
//! |-- platform-native/tests/fs_mutation.rs
//! |-- platform-native/tests/fs_util.rs
//! |-- platform-native/tests/fs_watcher.rs
//! |-- runtime/src/location_tools.rs
//! |-- tools/tests/files_common.rs
//! |-- tools/tests/files_mutation/native.rs
//! |-- tools/tests/files_mutation/support.rs
//! |-- tools/tests/files_read/support.rs
//! `-- tools/tests/shell_core.rs
//! Invariants: path resolution reads metadata only, not target bytes.
//! Contract tests: platform-native/tests/fs_mutation.rs

pub mod encoding;
mod error;
pub mod glob;
pub mod ignore;
pub mod location;
pub mod mutation;
pub mod native;
pub mod watcher;

pub use glob::{GlobOptions, glob_match};
pub use location::NativeLocationMutation;
pub use mutation::NativeFileMutation;
pub use native::NativeFileSystem;
pub use watcher::NativeWatcher;
