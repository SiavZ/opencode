//! Purpose: entrypoint for the plugin compatibility host. Filled in by p07.plugins.
//! Scope: binary entrypoint.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::Result
//! `-- anyhow::bail
//! Used by:
//! `-- none, entrypoint
//! Invariants: never runs first-party policy, only bridges user plugins.
//! Contract tests: none yet

fn main() -> anyhow::Result<()> {
    anyhow::bail!("compatibility host not yet ported")
}
