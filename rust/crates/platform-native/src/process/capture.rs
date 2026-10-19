//! Purpose: bounded output capture (`collectStream` in packages/core/src/process.ts:121-139) and
//! pipe readers that also forward every chunk to an optional live sink.
//! Scope: in-memory buffers and tokio pipe readers.
//! Uses:
//! `-- none
//! External:
//! |-- tokio::io::AsyncRead
//! |-- tokio::io::AsyncReadExt
//! `-- tokio::sync::mpsc::Sender
//! Used by:
//! rust/crates/
//! |-- platform-native/src/process/mod.rs
//! `-- platform-native/src/process/stream.rs
//! Invariants: at most `max` bytes are kept; `truncated` is set once more bytes than `max` arrived.
//! Contract tests: platform-native/tests/process.rs

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Default)]
pub struct Capture {
    pub bytes: Vec<u8>,
    seen: usize,
    pub truncated: bool,
    max: Option<usize>,
}

impl Capture {
    pub fn new(max: Option<usize>) -> Self {
        Self {
            max,
            ..Self::default()
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        let Some(max) = self.max else {
            self.bytes.extend_from_slice(chunk);
            self.seen += chunk.len();
            return;
        };
        let remaining = max.saturating_sub(self.seen);
        if remaining > 0 {
            self.bytes
                .extend_from_slice(&chunk[..remaining.min(chunk.len())]);
        }
        self.seen += chunk.len();
        self.truncated = self.truncated || self.seen > max;
    }
}

pub type Shared = Arc<Mutex<Capture>>;

pub fn shared(max: Option<usize>) -> Shared {
    Arc::new(Mutex::new(Capture::new(max)))
}

pub fn take(capture: &Shared) -> Capture {
    std::mem::take(&mut *capture.lock().unwrap_or_else(|poison| poison.into_inner()))
}

/// Read a pipe to EOF into `capture`, forwarding each chunk to `sink` first.
pub async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    capture: Shared,
    sink: Option<Sender<Vec<u8>>>,
) -> std::io::Result<()> {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        let chunk = &buffer[..read];
        capture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(chunk);
        if let Some(sink) = &sink {
            // A closed receiver only stops live forwarding; capture continues.
            let _ = sink.send(chunk.to_vec()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_marks_truncation() {
        let mut capture = Capture::new(Some(4));
        capture.push(b"ab");
        capture.push(b"cd");
        assert!(!capture.truncated);
        capture.push(b"e");
        assert_eq!(capture.bytes, b"abcd");
        assert!(capture.truncated);
        let mut open = Capture::new(None);
        open.push(b"abcdef");
        assert_eq!(open.bytes, b"abcdef");
        assert!(!open.truncated);
    }
}
