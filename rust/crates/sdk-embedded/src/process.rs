//! Purpose: child process termination and abort binding shared by the server and TUI launchers.
//! Port of packages/sdk/js/src/process.ts:5-30.
//! Scope: one spawned child process (and, for servers, its process group).
//! Uses:
//! `-- none
//! External:
//! |-- nix::sys::signal::Signal
//! |-- nix::sys::signal::kill
//! |-- nix::sys::signal::killpg
//! |-- nix::unistd::Pid
//! |-- tokio::process::Child
//! `-- tokio::process::Command
//! Used by:
//! rust/crates/
//! |-- sdk-embedded/src/launcher.rs
//! `-- sdk-embedded/src/lib.rs
//! Invariants: `stop` is a no-op for an exited child. Server children run in their own process
//! group on unix so `stop` terminates the whole tree, Windows uses `taskkill /T /F` first and
//! falls back to killing the pid, exactly like process.ts:7-12.
//! Contract tests: sdk-embedded/tests/launcher.rs

use tokio::process::{Child, Command};

/// How a child is terminated by [`stop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    /// The child leads its own process group: terminate the group (unix) or tree (windows).
    Group,
    /// Terminate only the pid on unix (TUI children must stay in the terminal's foreground
    /// group), still the full tree on windows like process.ts:9.
    Pid,
}

/// Prepare `command` so [`stop`] with [`Tree::Group`] can reach every descendant.
pub fn isolate(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Terminate `child` unless it already exited (process.ts:5-13). Sends SIGTERM on unix like
/// `ChildProcess.kill()`. Does not wait for the child.
pub fn stop(child: &mut Child, tree: Tree) {
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    let Some(pid) = child.id() else { return };
    #[cfg(windows)]
    {
        let _ = tree;
        let killed = std::process::Command::new("taskkill")
            .args(["/pid", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if killed {
            return;
        }
        let _ = child.start_kill();
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill, killpg};
        use nix::unistd::Pid;
        let pid = Pid::from_raw(pid as i32);
        let sent = match tree {
            Tree::Group => killpg(pid, Signal::SIGTERM),
            Tree::Pid => kill(pid, Signal::SIGTERM),
        };
        if sent.is_err() {
            let _ = child.start_kill();
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (tree, pid);
        let _ = child.start_kill();
    }
}
