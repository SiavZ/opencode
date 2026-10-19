//! Purpose: shell family: core V2 `bash` and legacy `bash` (shell) tools, shell selection, BashArity,
//! tree-sitter permission scan, prompt rendering and output bounding/retention.
//! Scope: module exports and bundled baseline schemas.
//! Uses:
//! rust/crates/
//! |-- tools/src/shell/arity.rs
//! |-- tools/src/shell/core.rs
//! |-- tools/src/shell/legacy.rs
//! |-- tools/src/shell/output.rs
//! |-- tools/src/shell/prompt.rs
//! |-- tools/src/shell/scan.rs
//! `-- tools/src/shell/select.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! |-- runtime/src/location_tools.rs
//! |-- tools/src/lib.rs
//! |-- tools/src/shell/core.rs
//! |-- tools/src/shell/legacy.rs
//! |-- tools/tests/shell_core.rs
//! |-- tools/tests/shell_legacy.rs
//! `-- tools/tests/shell_scan.rs
//! Invariants: the exposed tool ID and permission key stay `bash` in both generations (id.ts).
//! Contract tests: tools/tests/shell_core.rs, tools/tests/shell_legacy.rs, tools/tests/shell_scan.rs

pub mod arity;
pub mod core;
pub mod legacy;
pub mod output;
pub mod prompt;
pub mod scan;
pub mod select;

pub use self::core::CoreBashTool;
pub use self::legacy::{LegacyShellConfig, LegacyShellTool, NoShellEnv, ShellEnv};
pub use self::output::{DirectoryRetention, OutputRetention};

/// `ShellID.ToolID`: kept as `bash` for compatibility with plugins and saved permissions.
pub const TOOL_ID: &str = "bash";

pub mod schemas {
    //! Captured baseline JSON schemas for the bash tool in both generations.
    use serde_json::Value;
    use std::sync::OnceLock;

    pub const SCHEMAS: &str = include_str!("fixtures/schemas.json");
    pub const PROMPTS: &str = include_str!("fixtures/prompts.json");

    fn parsed() -> &'static Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        CELL.get_or_init(|| {
            serde_json::from_str(SCHEMAS).expect("validated bundled schema fixture")
        })
    }

    /// `kind` is `input` or `output`.
    pub fn core(kind: &str) -> Value {
        parsed()["core"][kind].clone()
    }

    pub fn legacy() -> Value {
        parsed()["legacy"].clone()
    }
}
