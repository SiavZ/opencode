//! Purpose: legacy Session operation paths (packages/opencode/src/session/prompt.ts `SessionPrompt`,
//! run-state.ts, revert.ts cleanup, share/session.ts, command/index.ts templating): the
//! `LegacySession` facade that the runtime composition wires into `ChildHost` (seam 3:
//! `prompt`/`cancel`/`resolvePromptParts`) and the legacy HTTP routes.
//! Scope: one `LegacySession` per Location instance. Model execution is delegated to `LegacyLoop`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/process.rs
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/compaction.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/policy.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/projectors.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/resource.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/share.rs
//! |-- session-runtime/src/legacy/shell.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/template.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/parts.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/resource.rs
//! |-- session-runtime/src/legacy/share.rs
//! |-- session-runtime/src/legacy/shell.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/tests/legacy_prompt.rs
//! `-- session-runtime/tests/support/legacy.rs
//! Invariants: every capability without a ported upstream adapter fails with typed
//! `LegacyError::NotYetPorted`; nothing silently succeeds. One run or shell per Session.
//! Contract tests: session-runtime/tests/legacy_prompt.rs, legacy_shell.rs, legacy_command.rs

pub mod command;
pub mod compaction;
mod parts;
pub mod policy;
pub mod ports;
pub mod projectors;
pub mod prompt;
mod resource;
pub mod run_state;
pub mod share;
pub mod shell;
pub mod store;
pub mod template;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;

use opencode_platform_api::fs::FileSystem;
use opencode_platform_api::process::ProcessRunner;

use ports::{LegacyAgents, LegacyCommands, LegacyLoop, LegacyProvider, PromptTools, ShareRemote};
use run_state::RunState;
use store::LegacyStore;

/// `InstanceState.context` fields the legacy paths read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instance {
    pub directory: PathBuf,
    pub worktree: PathBuf,
    /// `os.homedir()` for `~/` references.
    pub home: PathBuf,
}

/// Config reads of the legacy paths (`cfg.shell`, `cfg.share`, `flags.autoShare`, `cfg.snapshot`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LegacyConfig {
    /// Resolved preferred shell path (`Shell.preferred(cfg.shell)`).
    pub shell: String,
    /// `"manual" | "auto" | "disabled"`.
    pub share: Option<String>,
    pub auto_share: bool,
    pub snapshot: Option<bool>,
}

/// Collaborators of `LegacySession`.
#[derive(Clone)]
pub struct Deps {
    pub store: LegacyStore,
    pub run_state: RunState,
    pub agents: Arc<dyn LegacyAgents>,
    pub provider: Arc<dyn LegacyProvider>,
    pub run_loop: Arc<dyn LegacyLoop>,
    pub tools: Arc<dyn PromptTools>,
    pub commands: Arc<dyn LegacyCommands>,
    pub share: Arc<dyn ShareRemote>,
    pub fs: Arc<dyn FileSystem>,
    pub process: Arc<dyn ProcessRunner>,
    pub instance: Instance,
    pub config: LegacyConfig,
}

/// The legacy `SessionPrompt` service plus share/revert-cleanup entry points.
#[derive(Clone)]
pub struct LegacySession {
    deps: Deps,
}

impl LegacySession {
    pub fn new(deps: Deps) -> Self {
        Self { deps }
    }

    pub fn store(&self) -> &LegacyStore {
        &self.deps.store
    }

    pub fn run_state(&self) -> &RunState {
        &self.deps.run_state
    }
}
