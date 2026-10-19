//! Purpose: Core bounded text paging (`ReadToolFileSystem.read` paged branch): line offsets,
//! line/byte caps, long-line discard, CRLF trimming and per-segment binary checks.
//! Scope: content reads after approval.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- tools/src/files/read_common.rs
//! `-- tools/src/files/read_files.rs
//! External:
//! `-- usize::from
//! Used by:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! `-- tools/src/files/read_files.rs
//! Invariants: reading stops as soon as the page is complete, so bytes past it are never decoded
//! or binary-checked, only `\n` terminates lines and one trailing `\r` is trimmed.
//! Contract tests: tools/tests/files_read/filesystem.rs

use super::read_common::{
    CHUNK_BYTES, MAX_LINE_LENGTH, MAX_READ_BYTES, MAX_READ_LINES, Utf8Stream, is_binary, js_len,
    js_slice, truncate_line,
};
use super::read_files::{FileRead, Page, ReadError, TextPage};
use opencode_platform_api::fs::FileSystem;
use std::path::Path;

struct Pager {
    offset: u64,
    limit: usize,
    lines: Vec<String>,
    pending: String,
    discard: bool,
    line: u64,
    bytes: usize,
    next: Option<u64>,
}

impl Pager {
    fn full(&self) -> bool {
        self.lines.len() >= self.limit || self.bytes >= MAX_READ_BYTES
    }

    fn append(&mut self, input: &str) -> bool {
        if self.line < self.offset {
            self.line += 1;
            return true;
        }
        if self.full() {
            self.next = Some(self.line);
            return false;
        }
        let text = truncate_line(input);
        let size = text.len() + usize::from(!self.lines.is_empty());
        if self.bytes + size > MAX_READ_BYTES {
            self.next = Some(self.line);
            return false;
        }
        self.lines.push(text);
        self.bytes += size;
        self.line += 1;
        true
    }

    fn consume(&mut self, input: &str) -> bool {
        let mut text = input;
        while let Some(index) = text.find('\n') {
            let current = if self.discard {
                std::mem::take(&mut self.pending)
            } else {
                std::mem::take(&mut self.pending) + &text[..index]
            };
            self.discard = false;
            text = &text[index + 1..];
            if !self.append(current.strip_suffix('\r').unwrap_or(&current)) {
                return false;
            }
        }
        self.push_pending(text);
        true
    }

    fn push_pending(&mut self, text: &str) {
        if self.discard {
            return;
        }
        self.pending.push_str(text);
        if js_len(&self.pending) > MAX_LINE_LENGTH {
            self.pending = js_slice(&self.pending, MAX_LINE_LENGTH + 1);
            self.discard = true;
        }
    }
}

pub(super) async fn paged_text(
    fs: &dyn FileSystem,
    real: &Path,
    resource: &str,
    page: Page,
    first: Vec<u8>,
    mime: String,
) -> Result<FileRead, ReadError> {
    let offset = page.offset.unwrap_or(1);
    let mut pager = Pager {
        offset,
        limit: page
            .limit
            .map_or(MAX_READ_LINES, |limit| (limit as usize).min(MAX_READ_LINES)),
        lines: Vec::new(),
        pending: String::new(),
        discard: false,
        line: 1,
        bytes: 0,
        next: None,
    };
    let mut decoder = Utf8Stream::new(true);
    let malformed = |_| ReadError::MalformedUtf8 {
        resource: resource.into(),
    };
    let mut chunk = first;
    let mut position = 0u64;
    let mut done = false;
    while !done && !chunk.is_empty() {
        position += chunk.len() as u64;
        let mut start = 0;
        while start < chunk.len() {
            if pager.full() {
                pager.next = Some(pager.line);
                done = true;
                break;
            }
            let end = chunk[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(chunk.len(), |index| start + index + 1);
            let segment = &chunk[start..end];
            if is_binary(resource, segment) {
                return Err(ReadError::Binary {
                    resource: resource.into(),
                });
            }
            if !pager.consume(&decoder.push(segment).map_err(malformed)?) {
                done = true;
                break;
            }
            start = end;
        }
        if !done {
            chunk = fs.read_range(real, position, CHUNK_BYTES).await?;
        }
    }
    if !done {
        let tail = decoder.finish().map_err(malformed)?;
        if !pager.discard {
            pager.pending.push_str(&tail);
        }
        if !pager.pending.is_empty() {
            let pending = std::mem::take(&mut pager.pending);
            pager.append(pending.strip_suffix('\r').unwrap_or(&pending));
        }
    }
    if pager.lines.is_empty() && offset != 1 {
        return Err(ReadError::OffsetOutOfRange { offset });
    }
    Ok(FileRead::Page(TextPage {
        kind: "text-page",
        content: pager.lines.join("\n"),
        mime,
        offset,
        truncated: pager.next.is_some(),
        next: pager.next,
    }))
}
