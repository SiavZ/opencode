//! Purpose: Legacy streaming line window (`ReadTool.lines`): Effect `splitLines` semantics over a
//! non-fatal UTF-8 decode, line/byte caps and early stop at the byte cap.
//! Scope: content read after approval.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! `-- tools/src/files/read_common.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- tools/src/files/read_legacy.rs
//! Invariants: `\n`, `\r\n` and lone `\r` terminate lines, a final unterminated line is kept,
//! an incomplete trailing UTF-8 sequence is dropped (decoder never flushed), counting continues
//! past the line limit but stops at the byte cap.
//! Contract tests: tools/tests/files_read.rs

use super::read_common::{CHUNK_BYTES, MAX_READ_BYTES, Utf8Stream, truncate_line};
use opencode_platform_api::PlatformError;
use opencode_platform_api::fs::FileSystem;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Default)]
pub struct Lines {
    pub raw: Vec<String>,
    pub count: u64,
    pub cut: bool,
    pub more: bool,
}

struct State {
    window: Window,
    lines: Lines,
    bytes: usize,
    done: bool,
}

impl State {
    /// Returns false once the byte cap stops the stream.
    fn line(&mut self, text: &str) -> bool {
        if self.done {
            return false;
        }
        self.lines.count += 1;
        if self.lines.count < self.window.offset {
            return true;
        }
        if self.lines.raw.len() as u64 >= self.window.limit {
            self.lines.more = true;
            return true;
        }
        let line = truncate_line(text);
        let size = line.len() + usize::from(!self.lines.raw.is_empty());
        if self.bytes + size <= MAX_READ_BYTES {
            self.lines.raw.push(line);
            self.bytes += size;
            return true;
        }
        self.lines.cut = true;
        self.lines.more = true;
        self.done = true;
        false
    }
}

pub async fn lines(
    fs: &dyn FileSystem,
    path: &Path,
    window: Window,
) -> Result<Lines, PlatformError> {
    let mut state = State {
        window,
        lines: Lines::default(),
        bytes: 0,
        done: false,
    };
    let mut decoder = Utf8Stream::new(false);
    let mut builder = String::new();
    let mut mid_crlf = false;
    let mut position = 0u64;
    'stream: loop {
        let chunk = fs.read_range(path, position, CHUNK_BYTES).await?;
        if chunk.is_empty() {
            break;
        }
        position += chunk.len() as u64;
        let text = decoder.push(&chunk).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let mut rest = text.as_str();
        if mid_crlf {
            mid_crlf = false;
            if !state.line(&std::mem::take(&mut builder)) {
                break 'stream;
            }
            rest = rest.strip_prefix('\n').unwrap_or(rest);
        }
        while let Some(index) = rest.find(['\r', '\n']) {
            if rest.as_bytes()[index] == b'\r' && index + 1 == rest.len() {
                builder.push_str(&rest[..index]);
                mid_crlf = true;
                rest = "";
                break;
            }
            builder.push_str(&rest[..index]);
            let width = if rest[index..].starts_with("\r\n") {
                2
            } else {
                1
            };
            rest = &rest[index + width..];
            if !state.line(&std::mem::take(&mut builder)) {
                break 'stream;
            }
        }
        builder.push_str(rest);
    }
    if !state.done && (!builder.is_empty() || mid_crlf) {
        state.line(&builder);
    }
    Ok(state.lines)
}
