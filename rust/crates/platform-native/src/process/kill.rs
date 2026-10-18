//! Purpose: process-group termination with SIGTERM, bounded grace and SIGKILL escalation, plus a
//! drop guard that kills an abandoned group (cross-spawn-spawner.ts killGroup/killOne/finalizer).
//! Scope: unix process groups via nix, Windows tree kill via taskkill.
//! Uses:
//! `-- none
//! External:
//! |-- nix::sys::signal::{killpg, kill, Signal}
//! `-- tokio::process::Child
//! Used by:
//! rust/crates/
//! |-- platform-native/src/process/mod.rs
//! `-- platform-native/src/process/stream.rs
//! Invariants: termination waits for the leader to be reaped, and for the whole group to vanish
//! or the grace period to end before escalating to SIGKILL of the group.
//! Contract tests: platform-native/tests/process.rs

use std::time::Duration;
use tokio::process::Child;

/// Default `forceKillAfter` used by the baseline shell tools.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);

/// Identity of a spawned child for signalling after it may have been reaped.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub pid: u32,
    pub group: bool,
}

#[cfg(unix)]
fn send(target: Target, signal: nix::sys::signal::Signal) -> bool {
    use nix::sys::signal::{kill, killpg};
    use nix::unistd::Pid;
    let Ok(raw) = i32::try_from(target.pid) else {
        return false;
    };
    let pid = Pid::from_raw(raw);
    if target.group && killpg(pid, signal).is_ok() {
        return true;
    }
    kill(pid, signal).is_ok()
}

/// Whether any member of the group (or the single process) still exists.
#[cfg(unix)]
pub fn alive(target: Target) -> bool {
    use nix::sys::signal::{kill, killpg};
    use nix::unistd::Pid;
    let Ok(raw) = i32::try_from(target.pid) else {
        return false;
    };
    let pid = Pid::from_raw(raw);
    if target.group {
        return killpg(pid, None).is_ok();
    }
    kill(pid, None).is_ok()
}

#[cfg(windows)]
pub fn alive(_target: Target) -> bool {
    false
}

/// Fire-and-forget SIGKILL of the group, used by drop guards and after-exit cleanup.
pub fn kill_now(target: Target) {
    #[cfg(unix)]
    {
        send(target, nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(windows)]
    {
        taskkill(target.pid);
    }
}

#[cfg(windows)]
fn taskkill(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/pid", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// SIGTERM the group, wait for the leader and the group up to `grace`, then SIGKILL the group and
/// reap the leader. Returns the leader's exit status when it could be observed.
pub async fn terminate(
    child: &mut Child,
    target: Target,
    grace: Duration,
) -> Option<std::process::ExitStatus> {
    #[cfg(windows)]
    {
        let _ = grace;
        taskkill(target.pid);
        let _ = child.start_kill();
        return child.wait().await.ok();
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::Signal;
        if !send(target, Signal::SIGTERM) {
            let _ = child.start_kill();
        }
        let deadline = tokio::time::Instant::now() + grace;
        let mut status = None;
        loop {
            if status.is_none()
                && let Ok(Some(done)) = child.try_wait()
            {
                status = Some(done);
            }
            if status.is_some() && !alive(target) {
                return status;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        send(target, Signal::SIGKILL);
        let _ = child.start_kill();
        match status {
            Some(status) => Some(status),
            None => child.wait().await.ok(),
        }
    }
}

/// Kills the group on drop unless disarmed, so an abandoned run never leaks its process tree.
#[derive(Debug)]
pub struct GroupGuard {
    target: Option<Target>,
}

impl GroupGuard {
    pub fn new(target: Target) -> Self {
        Self {
            target: Some(target),
        }
    }

    pub fn disarm(&mut self) {
        self.target = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(target) = self.target.take() {
            kill_now(target);
        }
    }
}
