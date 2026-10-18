//! Purpose: isolate spawned children in their own process group and signal or kill the whole
//! tree, so stopping an owned server never leaves surviving descendants.
//! Scope: OS process-group adapter (unix process groups via `kill`, Windows via `taskkill /T`).
//! Uses:
//! `-- none
//! External:
//! `-- tokio::process::{Child, Command}
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections/child.rs
//! |-- desktop-host/src/platform/shell_env.rs
//! `-- desktop-host/tests/connections_live.rs
//! Invariants: `terminate_tree` always reaps the leader (joined wait) and finishes with a KILL sweep
//! of the group, so descendants that ignored the graceful signal do not survive.
//! Contract tests: desktop-host/tests/connections_live.rs

use std::time::Duration;

use tokio::process::{Child, Command};

/// Signal sent to a process tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSignal {
    /// Graceful stop request (SIGTERM, or `taskkill /T` without `/F`).
    Terminate,
    /// Forced kill (SIGKILL, or `taskkill /T /F`).
    Kill,
}

/// Exit details of a reaped child, mirroring Node's `(code, signal)` exit pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

impl ExitInfo {
    pub fn from_status(status: std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            Self {
                code: status.code(),
                signal: status.signal().map(signal_name),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                code: status.code(),
                signal: None,
            }
        }
    }

    /// Node renders missing values as the literal `null` in desktop error messages.
    pub fn code_text(&self) -> String {
        self.code
            .map_or_else(|| "null".to_string(), |code| code.to_string())
    }

    pub fn signal_text(&self) -> String {
        self.signal.clone().unwrap_or_else(|| "null".to_string())
    }
}

/// Put the command's child in a fresh process group (unix) or new process group without a
/// console window (Windows) so the tree can be signalled as a unit.
pub fn isolate(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command.kill_on_drop(true);
}

/// Signal every process in the tree rooted at `pid` (the group created by [`isolate`]).
/// Errors are ignored: the group may already be gone.
pub async fn signal_tree(pid: u32, signal: TreeSignal) {
    #[cfg(unix)]
    {
        let flag = match signal {
            TreeSignal::Terminate => "-TERM",
            TreeSignal::Kill => "-KILL",
        };
        let _ = Command::new("kill")
            .args([flag, "--", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/T", "/PID", &pid.to_string()]);
        if signal == TreeSignal::Kill {
            command.arg("/F");
        }
        let _ = command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}

/// Graceful stop then joined kill: send `Terminate` to the tree, wait up to `grace` for the leader,
/// force-kill on timeout, always reap the leader, then sweep the group with `Kill`.
pub async fn terminate_tree(child: &mut Child, grace: Duration) -> Option<ExitInfo> {
    let pid = child.id();
    if let Some(pid) = pid {
        signal_tree(pid, TreeSignal::Terminate).await;
    }
    let status = match tokio::time::timeout(grace, child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => kill_and_reap(child, pid).await,
    };
    if let Some(pid) = pid {
        signal_tree(pid, TreeSignal::Kill).await;
    }
    status.map(ExitInfo::from_status)
}

/// Immediate kill of the tree, then joined reap of the leader.
pub async fn kill_tree(child: &mut Child) -> Option<ExitInfo> {
    let pid = child.id();
    let status = kill_and_reap(child, pid).await;
    status.map(ExitInfo::from_status)
}

async fn kill_and_reap(child: &mut Child, pid: Option<u32>) -> Option<std::process::ExitStatus> {
    if let Some(pid) = pid {
        signal_tree(pid, TreeSignal::Kill).await;
    }
    let _ = child.start_kill();
    child.wait().await.ok()
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return format!("SIG{signal}"),
    }
    .to_string()
}
