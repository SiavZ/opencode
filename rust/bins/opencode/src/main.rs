//! Purpose: `opencode` executable composition root: parse argv with the yargs-compatible spec,
//! render help/version/completion outcomes, apply the global middleware env side effects
//! (packages/opencode/src/index.ts:66-78) and hand the typed command to dispatch.
//! Scope: process entrypoint.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/mod.rs
//! `-- opencode/src/cli/spec/mod.rs
//! External:
//! |-- std::io::IsTerminal
//! `-- std::process::ExitCode
//! Used by:
//! `-- none, entrypoint
//! Invariants: help and failures go to stderr; failures exit 1; never claims a capability it
//! does not implement (unported commands exit 2 with a NotYetPorted message).
//! Contract tests: opencode/tests/cli_help.rs
mod cli;
mod serve;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use cli::spec::{self, Invocation, ParseEnv};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let shell = |k: &str| std::env::var(k).unwrap_or_default().contains("zsh");
    let env = ParseEnv {
        version: option_env!("OPENCODE_VERSION")
            .unwrap_or("local")
            .to_string(),
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        tty: std::io::stdout().is_terminal() || std::io::stderr().is_terminal(),
        zsh: shell("SHELL") || shell("ZSH_NAME"),
    };
    match spec::parse(&argv, &env) {
        Invocation::Help { stderr, code } => {
            let _ = std::io::stderr().write_all(stderr.as_bytes());
            ExitCode::from(code)
        }
        Invocation::Fail { stderr } => {
            let _ = std::io::stderr().write_all(stderr.as_bytes());
            ExitCode::from(1)
        }
        Invocation::Version(text)
        | Invocation::CompletionScript(text)
        | Invocation::Completions(text) => {
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        Invocation::Run(parsed) => run(parsed),
    }
}

/// Hand-off to `cli::dispatch` (owner p09.cli-commands). Until it exposes `run(Parsed)`, every
/// command except the Phase 91 composed ones reports a typed NotYetPorted error. The global
/// middleware env (`parsed.globals.middleware_env(pid)`) is data, not `set_var` (unsafe in
/// edition 2024): dispatch passes it into the runtime config and child process environments.
fn run(parsed: spec::Parsed) -> ExitCode {
    let _env = parsed.globals.middleware_env(std::process::id());
    if parsed.command == spec::CommandId::Serve {
        return serve::run(&parsed, option_env!("OPENCODE_VERSION").unwrap_or("local"));
    }
    eprintln!(
        "opencode (rust port): command {:?} is not yet ported, see rust/contracts/phase-09/status.md",
        parsed.command
    );
    ExitCode::from(2)
}
