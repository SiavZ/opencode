//! Purpose: command line spec for the legacy (yargs) and v2 (effect cli) trees: model, help
//! rendering, yargs-compatible parsing, completion and the typed `Invocation` dispatch consumes.
//! Scope: process parse.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/argv.rs
//! |-- opencode/src/cli/spec/clap_tree.rs
//! |-- opencode/src/cli/spec/cliui.rs
//! |-- opencode/src/cli/spec/complete.rs
//! |-- opencode/src/cli/spec/help.rs
//! |-- opencode/src/cli/spec/invocation.rs
//! |-- opencode/src/cli/spec/model.rs
//! |-- opencode/src/cli/spec/parse.rs
//! |-- opencode/src/cli/spec/tree.rs
//! |-- opencode/src/cli/spec/tree_debug.rs
//! |-- opencode/src/cli/spec/tree_misc.rs
//! `-- opencode/src/cli/spec/v2.rs
//! External:
//! `-- clap::Command
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/mod.rs
//! `-- opencode/src/main.rs
//! Invariants: help text matches captured baseline fixtures (tests/fixtures/help).
//! Contract tests: opencode/tests/cli_help.rs, opencode/src/cli/spec/tests.rs
// Public API consumed by dispatch (p09.cli-commands) and the v2 executable; unused until wired.
#![allow(dead_code, unused_imports)]
mod argv;
mod clap_tree;
mod cliui;
mod complete;
mod help;
pub mod invocation;
pub mod model;
mod parse;
#[cfg(test)]
mod tests;
mod tree;
mod tree_debug;
mod tree_misc;
mod v2;

pub use invocation::{Args, CommandId, Globals, Invocation, Parsed, Value};
pub use parse::ParseEnv;

/// Parses the legacy `opencode` argv (without the executable) the way yargs 18 does.
pub fn parse(argv: &[String], env: &ParseEnv) -> Invocation {
    parse::parse(argv, env)
}

/// The legacy tree as a clap command (for clap_complete bash/zsh/fish generation and introspection).
pub fn command() -> clap::Command {
    clap_tree::build(&tree::root())
}

/// clap_complete script for `shell` (bash, zsh, fish). The baseline `completion` command emits the
/// yargs script instead; this is the native alternative.
pub fn clap_completion(shell: clap_complete::Shell) -> String {
    let mut cmd = command();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, "opencode", &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// v2 executable tree (packages/cli/src/commands/commands.ts).
pub fn v2_command() -> clap::Command {
    v2::command()
}

/// v2 help text for a command path; `None` for an unknown path.
pub fn v2_help(path: &[&str]) -> Option<String> {
    v2::help(path)
}
