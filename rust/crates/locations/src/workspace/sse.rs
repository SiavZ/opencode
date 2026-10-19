//! Purpose: `parseSSE` and `route` of packages/opencode/src/control-plane/workspace.ts: incremental
//! server-sent-event decoding into JSON events and URL joining for workspace endpoints.
//! Scope: pure decoding helpers.
//! Uses:
//! rust/crates/
//! `-- locations/src/error.rs
//! External:
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- locations/src/workspace/mod.rs
//! `-- locations/src/workspace/sync.rs
//! Invariants: lines split on LF, CRLF or CR; blank lines end events; `data` lines join with LF;
//! non-JSON data becomes `sse.message` with `data`, non-empty `id` and `retry` (default 1000);
//! trailing data is flushed at end of stream.
//! Contract tests: locations/tests/workspace_sync.rs

use serde_json::{Value as Json, json};

use crate::error::{LocationError, Result};

/// `route(url, path)`: append `path` to the URL path, drop query and fragment.
pub fn route(url: &str, path: &str) -> Result<String> {
    let mut next = url::Url::parse(url).map_err(|e| LocationError::Validation(e.to_string()))?;
    let joined = format!("{}{path}", next.path().trim_end_matches('/'));
    next.set_path(&joined);
    next.set_query(None);
    next.set_fragment(None);
    Ok(next.to_string())
}

/// Incremental `parseSSE` decoder: lines split on `\n`, `\r\n` or `\r`, events end at a blank line.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    skip_lf: bool,
    data: Vec<String>,
    id: Option<String>,
    retry: i64,
    started: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            retry: 1000,
            ..Self::default()
        }
    }

    /// Feed bytes, returning the decoded events in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Json> {
        if !self.started {
            self.started = true;
            self.retry = 1000;
        }
        let mut out = Vec::new();
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' | b'\r' => {
                    self.skip_lf = byte == b'\r';
                    let line =
                        String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
                    self.line(&line, &mut out);
                }
                other => self.buffer.push(other),
            }
        }
        out
    }

    /// End of stream (`onHalt`): flush the trailing line and any pending data.
    pub fn finish(&mut self) -> Vec<Json> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
            self.line(&line, &mut out);
        }
        if !self.data.is_empty() {
            out.push(self.event());
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<Json>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(self.event());
            }
            return;
        }
        let (field, value) = match line.find(':') {
            None => (line, ""),
            Some(index) => {
                let rest = &line[index + 1..];
                (&line[..index], rest.strip_prefix(' ').unwrap_or(rest))
            }
        };
        match field {
            "data" => self.data.push(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            "retry" => {
                if let Some(retry) = parse_int_prefix(value) {
                    self.retry = retry;
                }
            }
            _ => {}
        }
    }

    fn event(&mut self) -> Json {
        let data = std::mem::take(&mut self.data).join("\n");
        serde_json::from_str(&data).unwrap_or_else(|_| {
            let mut properties = serde_json::Map::new();
            properties.insert("data".into(), Json::String(data));
            if let Some(id) = self.id.as_ref().filter(|id| !id.is_empty()) {
                properties.insert("id".into(), Json::String(id.clone()));
            }
            properties.insert("retry".into(), json!(self.retry));
            json!({ "type": "sse.message", "properties": properties })
        })
    }
}

/// `Number.parseInt(value, 10)`: optional sign and leading digits after leading whitespace.
fn parse_int_prefix(value: &str) -> Option<i64> {
    let trimmed = value.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|n| sign * n)
}
