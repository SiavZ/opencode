//! Purpose: `runStream` (packages/core/src/process.ts:200-245): non-empty output lines, optional
//! stderr interleave, ok-exit-code check with bounded stderr, cancellation, group cleanup on drop.
//! Scope: tokio child + mpsc-backed stream.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! |-- platform-native/src/process/capture.rs
//! |-- platform-native/src/process/kill.rs
//! `-- platform-native/src/process/mod.rs
//! External:
//! |-- capture::pump
//! |-- capture::shared
//! |-- capture::take
//! |-- futures::stream::BoxStream
//! |-- futures::stream::StreamExt
//! |-- futures::stream::unfold
//! |-- kill::GroupGuard::new
//! |-- kill::Target
//! |-- mpsc::Receiver
//! |-- mpsc::Sender
//! |-- mpsc::channel
//! |-- tokio::io::AsyncBufReadExt
//! |-- tokio::io::AsyncRead
//! |-- tokio::io::BufReader
//! |-- tokio::select
//! |-- tokio::spawn
//! |-- tokio::sync::mpsc
//! `-- tokio::task::JoinHandle
//! Used by:
//! rust/crates/
//! `-- platform-native/src/process/mod.rs
//! Invariants: dropping the stream terminates the process group; the driver task is owned by the
//! stream and aborted with it.
//! Contract tests: platform-native/tests/process.rs

use super::{capture, exit_parts, kill, prepare, spawn_error};
use futures::stream::{BoxStream, StreamExt};
use opencode_platform_api::process::{Command, ProcessError, StreamOptions};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

type Item = Result<String, ProcessError>;

struct Owned {
    receiver: mpsc::Receiver<Item>,
    driver: tokio::task::JoinHandle<()>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        // Aborting drops the child and its GroupGuard, which kills the group.
        self.driver.abort();
    }
}

pub fn lines(command: Command, options: StreamOptions) -> BoxStream<'static, Item> {
    let (sender, receiver) = mpsc::channel::<Item>(64);
    let driver = tokio::spawn(drive(command, options, sender));
    futures::stream::unfold(Owned { receiver, driver }, |mut owned| async move {
        let item = owned.receiver.recv().await?;
        Some((item, owned))
    })
    .boxed()
}

async fn forward<R: tokio::io::AsyncRead + Unpin>(reader: R, sender: mpsc::Sender<Item>) {
    // Lossy decoding like the baseline `Stream.decodeText`, so invalid UTF-8 never stops reading.
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buffer);
        let line = text.trim_end_matches('\n');
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if sender.send(Ok(line.to_string())).await.is_err() {
            return;
        }
    }
}

async fn drive(command: Command, options: StreamOptions, sender: mpsc::Sender<Item>) {
    let description = command.describe();
    let spawned = prepare(&command, true, false).and_then(|mut prepared| {
        prepared
            .spawn()
            .map_err(|error| spawn_error(&command, &error))
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(Err(error)).await;
            return;
        }
    };
    let target = kill::Target {
        pid: child.id().unwrap_or_default(),
        group: true,
    };
    let mut guard = kill::GroupGuard::new(target);
    let stderr = capture::shared(options.max_error_bytes);
    let mut tasks = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        tasks.push(tokio::spawn(forward(pipe, sender.clone())));
    }
    if let Some(pipe) = child.stderr.take() {
        if options.include_stderr {
            tasks.push(tokio::spawn(forward(pipe, sender.clone())));
        } else {
            let stderr = stderr.clone();
            tasks.push(tokio::spawn(async move {
                let _ = capture::pump(pipe, stderr, None).await;
            }));
        }
    }
    let cancel = options.cancel.unwrap_or_default();
    let finished = async {
        for task in tasks.iter_mut() {
            if !task.is_finished() {
                let _ = task.await;
            }
        }
        child.wait().await
    };
    let status = tokio::select! {
        status = finished => status,
        _ = cancel.cancelled() => {
            kill::terminate(&mut child, target, kill::DEFAULT_GRACE).await;
            guard.disarm();
            for task in tasks {
                if task.is_finished() {
                    continue;
                }
                task.abort();
                let _ = task.await;
            }
            let _ = sender.send(Err(ProcessError::Cancelled { command: description })).await;
            return;
        }
    };
    guard.disarm();
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            let _ = sender.send(Err(spawn_error(&command, &error))).await;
            return;
        }
    };
    let (code, _) = exit_parts(status);
    if !options.ok_exit_codes.is_empty() && !options.ok_exit_codes.contains(&code) {
        let stderr = String::from_utf8_lossy(&capture::take(&stderr).bytes).into_owned();
        let _ = sender
            .send(Err(ProcessError::Failed {
                command: description,
                exit_code: Some(code),
                stderr: Some(stderr),
                detail: None,
            }))
            .await;
    }
}
