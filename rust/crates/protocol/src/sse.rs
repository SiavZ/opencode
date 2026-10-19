//! Purpose: SSE wire framing shared by server encoders and client decoders, byte-compatible with
//! Effect `unstable/encoding/Sse` `encoder.write` and the baseline stream header policy.
//! Scope: pure string encoding, no I/O, no timers.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- protocol/src/lib.rs
//! |-- protocol/tests/sse.rs
//! |-- server/src/handlers/streams.rs
//! |-- server/src/transport/sse.rs
//! `-- server/tests/sse_events.rs
//! Invariants: `id:` only when present, `event:` omitted for `message`, `data:` omitted when empty,
//! embedded `\n` split into multiple `data:` lines, frame terminated by a blank line.
//! Heartbeat cadence and header sets are protocol constants consumed by server transports.
//! Contract tests: protocol/tests/sse.rs

use std::time::Duration;

/// One SSE frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: Option<String>,
    /// SSE `event` field, `message` is the default and is not written.
    pub event: String,
    pub data: String,
}

impl Frame {
    pub fn message(data: impl Into<String>) -> Self {
        Frame {
            id: None,
            event: "message".to_string(),
            data: data.into(),
        }
    }

    /// Encode exactly like Effect's default SSE encoder.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        if let Some(id) = &self.id {
            out.push_str("id: ");
            out.push_str(id);
            out.push('\n');
        }
        if self.event != "message" {
            out.push_str("event: ");
            out.push_str(&self.event);
            out.push('\n');
        }
        if !self.data.is_empty() {
            out.push_str("data: ");
            out.push_str(&self.data.replace('\n', "\ndata: "));
            out.push('\n');
        }
        out.push('\n');
        out
    }
}

/// `retry:` directive frame.
pub fn retry(duration: Duration) -> String {
    format!("retry: {}\n\n", duration.as_millis())
}

/// Current `/api/event` comment heartbeat (`packages/server/src/handlers/event.ts`).
pub const CURRENT_HEARTBEAT: &str = ": heartbeat\n\n";
/// Current global stream heartbeat period.
pub const CURRENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// Current global stream subscriber capacity, overflow fails the subscriber.
pub const CURRENT_SUBSCRIBER_CAPACITY: usize = 256;
/// Legacy `/event` JSON heartbeat period, first tick dropped, frame type `server.heartbeat`.
pub const LEGACY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
pub const LEGACY_HEARTBEAT_TYPE: &str = "server.heartbeat";
pub const CONNECTED_TYPE: &str = "server.connected";
pub const INSTANCE_DISPOSED_TYPE: &str = "server.instance.disposed";

/// Response headers on every baseline SSE stream, in baseline order, after content type.
pub const CONTENT_TYPE: &str = "text/event-stream";
pub const HEADERS: [(&str, &str); 3] = [
    ("Cache-Control", "no-cache, no-transform"),
    ("X-Accel-Buffering", "no"),
    ("X-Content-Type-Options", "nosniff"),
];

/// Incremental decoder for SSE byte streams (split CRLF, multiline data, comments).
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: String,
    id: Option<String>,
    event: Option<String>,
    data: Option<Vec<String>>,
}

impl Decoder {
    /// Feed a text chunk, returning completed frames. Comments (`:`) are ignored.
    pub fn push(&mut self, chunk: &str) -> Vec<Frame> {
        self.buffer.push_str(chunk);
        let mut frames = Vec::new();
        while let Some(end) = self.buffer.find(['\n', '\r']) {
            // A lone trailing `\r` may be the first half of a split CRLF, wait for more input.
            if self.buffer.as_bytes()[end] == b'\r' && end + 1 == self.buffer.len() {
                break;
            }
            let line = self.buffer[..end].to_string();
            let skip = if self.buffer[end..].starts_with("\r\n") {
                2
            } else {
                1
            };
            self.buffer.drain(..end + skip);
            if let Some(frame) = self.line(&line) {
                frames.push(frame);
            }
        }
        frames
    }

    fn line(&mut self, line: &str) -> Option<Frame> {
        if line.is_empty() {
            let data = self.data.take()?;
            return Some(Frame {
                id: self.id.take(),
                event: self.event.take().unwrap_or_else(|| "message".to_string()),
                data: data.join("\n"),
            });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => {
                self.data
                    .get_or_insert_with(Vec::new)
                    .push(value.to_string());
            }
            "event" => self.event = Some(value.to_string()),
            "id" => self.id = Some(value.to_string()),
            _ => {}
        }
        None
    }
}
