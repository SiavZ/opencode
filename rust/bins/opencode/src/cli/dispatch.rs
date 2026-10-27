//! Purpose: dispatch parsed end-user commands to the production process and terminal handlers.
//! Scope: executable lifetime.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! `-- opencode/src/cli/spec/mod.rs
//! External:
//! |-- anyhow::Error
//! |-- anyhow::Error::from
//! |-- commands::run
//! `-- tokio::runtime::Builder::new_multi_thread
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/mod.rs
//! `-- opencode/src/main.rs
//! Invariants: unfinished capabilities fail explicitly, never report placeholder success.
//! Contract tests: opencode/tests/commands.rs

use std::process::ExitCode;

use super::commands;
use super::spec::{CommandId, Parsed};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("NotYetPorted: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

pub fn run(parsed: &Parsed, version: &str) -> ExitCode {
    if !matches!(
        parsed.command,
        CommandId::Run
            | CommandId::Tui
            | CommandId::Models
            | CommandId::DebugPaths
            | CommandId::DebugWait
            | CommandId::SessionList
            | CommandId::SessionDelete
            | CommandId::Import
            | CommandId::Export
            | CommandId::AgentList
            | CommandId::DbQuery
            | CommandId::DbPath
            | CommandId::DebugConfig
            | CommandId::McpList
            | CommandId::McpAuthList
            | CommandId::McpLogout
            | CommandId::DebugSkill
            | CommandId::ProvidersList
            | CommandId::ProvidersLogout
    ) {
        eprintln!(
            "{}",
            CommandError::Unsupported(format!("command {:?}", parsed.command))
        );
        return ExitCode::from(2);
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .map_err(CommandError::from)
        .and_then(|runtime| runtime.block_on(commands::run(parsed, version)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::Unsupported(error)) => {
            eprintln!("NotYetPorted: {error}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(1)
        }
    }
}
