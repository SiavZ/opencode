//! Launch the Rust-defined plugin bridge with an explicitly chosen JS interpreter.
//! Policy and plugin selection remain in the Rust integration adapter.

use anyhow::{Context, bail};
use opencode_integration_api::plugin_abi::bridge_script;
use std::process::Command;

fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(interpreter) = arguments.next() else {
        bail!("usage: opencode-compatibility-host <bun-or-node>");
    };
    if interpreter == "--help" || interpreter == "-h" {
        println!("usage: opencode-compatibility-host <bun-or-node>");
        return Ok(());
    }
    if arguments.next().is_some() {
        bail!("usage: opencode-compatibility-host <bun-or-node>");
    }
    let mut command = Command::new(interpreter);
    command.args(["--input-type=module", "--eval", &bridge_script()]);

    // Replacing the launcher preserves stdio, signals, and the interpreter's exit status.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("could not launch plugin interpreter")
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .context("could not launch plugin interpreter")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
