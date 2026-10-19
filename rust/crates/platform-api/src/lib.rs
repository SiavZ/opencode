//! Purpose: crate root for the portable platform contract: filesystem identity and mutation,
//! processes, PTY sessions, watchers, git, snapshots and path containment. No native I/O.
//! Scope: traits, value types and pure path helpers shared by tools, config, storage and runtime.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/error.rs
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/path.rs
//! |-- platform-api/src/process.rs
//! |-- platform-api/src/pty.rs
//! |-- platform-api/src/snapshot.rs
//! `-- platform-api/src/watcher.rs
//! External:
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/error.rs
//! |-- platform-native/src/fs/glob.rs
//! |-- platform-native/src/fs/location.rs
//! |-- platform-native/src/fs/mutation.rs
//! |-- platform-native/src/fs/native.rs
//! |-- platform-native/src/fs/watcher.rs
//! |-- platform-native/tests/fs_location.rs
//! |-- platform-native/tests/fs_util.rs
//! |-- tools/src/files/read_files.rs
//! |-- tools/src/files/read_legacy.rs
//! |-- tools/src/files/read_lines.rs
//! |-- tools/tests/files_mutation/dev_mutation.rs
//! |-- tools/tests/files_mutation/support.rs
//! `-- tools/tests/files_read/support.rs
//! Invariants: compiles for wasm32-unknown-unknown, never depends on native-only crates.
//! Contract tests: platform-api/src/path.rs

pub mod error;
pub mod fs;
pub mod git;
pub mod path;
pub mod process;
pub mod pty;
pub mod snapshot;
pub mod watcher;

pub use error::{PlatformError, PlatformErrorKind};
pub use tokio_util::sync::CancellationToken;
