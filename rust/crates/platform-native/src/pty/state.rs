//! Purpose: retain PTY output and bounded subscriber state with UTF-16 absolute cursors.
//! Scope: process-local synchronized session state, shared by the reader and attachments.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! `-- platform-native/src/process/kill.rs
//! External:
//! |-- portable_pty::MasterPty
//! `-- tokio::sync::Notify
//! Used by:
//! rust/crates/
//! |-- platform-native/src/pty/attachment.rs
//! `-- platform-native/src/pty/mod.rs
//! Invariants: slow consumers end explicitly; replay never splits a UTF-8 character; teardown kills the process group.
//! Contract tests: platform-native/tests/pty.rs

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use opencode_platform_api::pty::{BUFFER_LIMIT, PtyInfo, PtyOutput, PtyStatus};
use portable_pty::{ChildKiller, MasterPty};
use tokio::sync::Notify;

pub(super) struct Subscriber {
    pub queue: Mutex<(VecDeque<PtyOutput>, usize, bool)>,
    pub ready: Notify,
}

impl Subscriber {
    pub fn push(&self, output: PtyOutput) {
        let mut state = lock(&self.queue);
        if state.2 {
            return;
        }
        let size = match &output {
            PtyOutput::Data(text) => text.len(),
            PtyOutput::End { .. } => 0,
        };
        if state.1 + size > BUFFER_LIMIT * 2 {
            state.0.clear();
            state.0.push_back(PtyOutput::End { exit_code: None });
            state.1 = 0;
            state.2 = true;
        } else {
            state.1 += size;
            state.2 = matches!(output, PtyOutput::End { .. });
            state.0.push_back(output);
        }
        drop(state);
        self.ready.notify_one();
    }
}

pub(super) struct State {
    pub info: PtyInfo,
    pub buffer: VecDeque<u16>,
    pub cursor: u64,
    pub subscribers: BTreeMap<u64, Arc<Subscriber>>,
    pub next: u64,
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Option<std::sync::mpsc::SyncSender<String>>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

impl State {
    pub fn data(&mut self, text: String) {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.cursor += units.len() as u64;
        self.buffer.extend(units);
        if self.buffer.len() > BUFFER_LIMIT {
            self.buffer.drain(..self.buffer.len() - BUFFER_LIMIT);
        }
        self.subscribers.retain(|_, subscriber| {
            subscriber.push(PtyOutput::Data(text.clone()));
            !lock(&subscriber.queue).2
        });
    }

    pub fn end(&mut self, exit_code: Option<u32>) {
        self.info.status = PtyStatus::Exited;
        self.info.exit_code = exit_code;
        for subscriber in self.subscribers.values() {
            subscriber.push(PtyOutput::End { exit_code });
        }
        self.subscribers.clear();
    }

    pub fn kill(&mut self) {
        self.writer.take();
        if self.info.status != PtyStatus::Running {
            return;
        }
        if self.info.pid > 0 {
            crate::process::kill::kill_now(crate::process::kill::Target {
                pid: self.info.pid,
                group: true,
            });
        }
        let _ = self.killer.kill();
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if self.info.status == PtyStatus::Running {
            self.kill();
        }
    }
}

pub(super) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
