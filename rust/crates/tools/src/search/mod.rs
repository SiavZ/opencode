//! Purpose: search family (glob, grep) for both tool generations, the ripgrep adapter and binary
//! resolution, plus bundled baseline schemas/descriptions.
//! Scope: module exports and the shared `SearchServices` dependency bundle.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! |-- tools/src/search/binary.rs
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/input.rs
//! |-- tools/src/search/legacy.rs
//! `-- tools/src/search/ripgrep.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/lib.rs
//! |-- tools/tests/search_ripgrep.rs
//! `-- tools/tests/search_tools.rs
//! Invariants: schemas are captured baseline producer output (fixtures/README.md); legacy
//! descriptions are byte-identical copies of packages/opencode/src/tool/{glob,grep}.txt.
//! Contract tests: tools/tests/search_tools.rs

pub mod binary;
pub mod core;
pub mod input;
pub mod legacy;
pub mod ripgrep;

pub use self::binary::{NoInstaller, RipgrepBinary, RipgrepInstaller};
pub use self::core::{CoreGlobTool, CoreGrepTool};
pub use self::legacy::{LegacyGlobTool, LegacyGrepTool};
pub use self::ripgrep::{Ripgrep, RipgrepError};

use std::path::PathBuf;

/// Location-scoped search dependencies.
#[derive(Debug, Clone)]
pub struct SearchServices {
    pub ripgrep: Ripgrep,
    /// Active Location / instance directory.
    pub directory: PathBuf,
    /// Instance worktree (legacy titles and external-directory containment).
    pub worktree: PathBuf,
}

pub mod schemas {
    //! Captured baseline JSON schemas for glob and grep.
    use serde_json::Value;
    use std::sync::OnceLock;

    pub const CORE: &str = include_str!("fixtures/core-schemas.json");
    pub const LEGACY: &str = include_str!("fixtures/legacy-schemas.json");

    fn parsed(source: &'static str, cell: &'static OnceLock<Value>) -> &'static Value {
        cell.get_or_init(|| serde_json::from_str(source).expect("validated bundled schema fixture"))
    }

    /// `kind` is `input` or `output`.
    pub fn core(name: &str, kind: &str) -> Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        parsed(CORE, &CELL)[name][kind].clone()
    }

    pub fn legacy(name: &str) -> Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        parsed(LEGACY, &CELL)[name].clone()
    }
}
