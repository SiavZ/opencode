//! Purpose: legacy shell output bounding helpers (packages/opencode/src/tool/shell.ts:217-249):
//! metadata `preview`, line/byte `tail`, an incremental UTF-8 decoder for streamed chunks, and the
//! retained-output port (`Truncate.write` / `Truncate.limits`) with a directory-backed adapter.
//! Scope: pure text helpers plus file writes under the managed tool-output directory.
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! |-- tokio::fs::OpenOptions::new
//! |-- tokio::fs::create_dir_all
//! |-- tokio::fs::write
//! |-- tokio::io::AsyncWriteExt
//! `-- usize::from
//! Used by:
//! rust/crates/
//! |-- tools/src/shell/legacy.rs
//! |-- tools/src/shell/mod.rs
//! `-- tools/tests/shell_legacy.rs
//! Invariants: preview keeps the last 30,000 UTF-16 units; tail keeps whole trailing lines within
//! both limits and never splits a UTF-8 sequence; retained files are named `tool_<ascending id>`.
//! Contract tests: tools/tests/shell_legacy.rs

use async_trait::async_trait;
use std::path::{Path, PathBuf};

pub const MAX_METADATA_LENGTH: usize = 30_000;
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 50 * 1024;

/// Retained full output (`Truncate.Service` subset used by the shell tool).
#[async_trait]
pub trait OutputRetention: Send + Sync + std::fmt::Debug {
    fn limits(&self) -> (usize, usize);
    /// Persist `text` and return the absolute file path.
    async fn write(&self, text: &str) -> Result<PathBuf, String>;
}

/// `Truncate.write` into `<data>/tool-output/tool_<ascending>` with configured limits.
#[derive(Debug, Clone)]
pub struct DirectoryRetention {
    pub directory: PathBuf,
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl DirectoryRetention {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            max_lines: MAX_LINES,
            max_bytes: MAX_BYTES,
        }
    }
}

#[async_trait]
impl OutputRetention for DirectoryRetention {
    fn limits(&self) -> (usize, usize) {
        (self.max_lines, self.max_bytes)
    }
    async fn write(&self, text: &str) -> Result<PathBuf, String> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(|error| error.to_string())?;
        let file = self
            .directory
            .join(format!("tool_{}", opencode_schema::identifier::ascending()));
        tokio::fs::write(&file, text)
            .await
            .map_err(|error| error.to_string())?;
        Ok(file)
    }
}

/// Append to a retained file (the baseline `createWriteStream(file, {flags: "a"})`).
pub async fn append(file: &Path, text: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut handle = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .await?;
    handle.write_all(text.as_bytes()).await?;
    handle.flush().await
}

/// `preview`: last 30,000 UTF-16 units, prefixed with `...\n\n` when cut.
pub fn preview(text: &str) -> String {
    let units = text.encode_utf16().count();
    if units <= MAX_METADATA_LENGTH {
        return text.to_string();
    }
    let skip = units - MAX_METADATA_LENGTH;
    let mut seen = 0;
    let mut start = text.len();
    for (index, ch) in text.char_indices() {
        if seen >= skip {
            start = index;
            break;
        }
        seen += ch.len_utf16();
    }
    format!("...\n\n{}", &text[start..])
}

/// `tail(text, maxLines, maxBytes)` -> (text, cut).
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= max_lines && text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev() {
        if out.len() >= max_lines {
            break;
        }
        let size = line.len() + usize::from(!out.is_empty());
        if bytes + size > max_bytes {
            if out.is_empty() {
                let raw = line.as_bytes();
                let mut start = raw.len().saturating_sub(max_bytes);
                while start < raw.len() && (raw[start] & 0xc0) == 0x80 {
                    start += 1;
                }
                out.push(String::from_utf8_lossy(&raw[start..]).into_owned());
            }
            break;
        }
        out.insert(0, (*line).to_string());
        bytes += size;
    }
    (out.join("\n"), true)
}

/// Streaming UTF-8 decoder (`Stream.decodeText`): incomplete trailing sequences wait for the next
/// chunk, invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    out.push_str(text);
                    self.pending.clear();
                    return out;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match error.error_len() {
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                        Some(len) => {
                            out.push('\u{FFFD}');
                            self.pending.drain(..valid + len);
                        }
                    }
                }
            }
        }
    }

    pub fn finish(&mut self) -> String {
        let rest = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_trailing_lines_and_utf8_boundaries() {
        assert_eq!(tail("a\nb\nc", 5, 100), ("a\nb\nc".into(), false));
        assert_eq!(tail("a\nb\nc", 2, 100), ("b\nc".into(), true));
        assert_eq!(tail("aaaa\nbb", 5, 4), ("bb".into(), true));
        assert_eq!(tail("xé", 10, 1), (String::new(), true));
        assert_eq!(tail("abcdef", 10, 3), ("def".into(), true));
    }

    #[test]
    fn preview_and_decoder() {
        let long = "x".repeat(30_005);
        assert_eq!(preview(&long), format!("...\n\n{}", "x".repeat(30_000)));
        let mut decoder = Utf8Decoder::default();
        let bytes = "é!".as_bytes();
        assert_eq!(decoder.push(&bytes[..1]), "");
        assert_eq!(decoder.push(&bytes[1..]), "é!");
        assert_eq!(decoder.push(&[0xff, b'a']), "\u{FFFD}a");
    }
}
