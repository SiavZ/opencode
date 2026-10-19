//! Purpose: incremental text/event-stream parser shared by the streamable HTTP and legacy SSE
//! transports (rmcp dropped the legacy client, so the SSE wire is Rust-owned here).
//! Scope: pure byte-to-event parsing plus a helper that pumps a reqwest body into events.
//! Uses:
//! `-- none
//! External:
//! |-- bytes::Bytes
//! |-- futures::Stream
//! `-- futures::StreamExt
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/legacy_sse.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/mcp/streamable.rs
//! Invariants: follows the WHATWG event-stream rules: `\n`, `\r\n` and `\r` line endings, a
//! single leading space after `:` is stripped, comments ignored, events dispatch on blank lines,
//! empty-data events are not dispatched, default event type is `message`.
//! Contract tests: integration-adapters/src/mcp/sse.rs (tests module)

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    pending_cr: bool,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    retry: Option<u64>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes and return every event completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for &b in chunk {
            if self.pending_cr {
                self.pending_cr = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.pending_cr = true;
                    self.line(&mut out);
                }
                b'\n' => self.line(&mut out),
                b => self.buffer.push(b),
            }
        }
        out
    }

    fn line(&mut self, out: &mut Vec<SseEvent>) {
        // Lines are complete here, so multi-byte characters split across chunks are intact.
        let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
        if line.is_empty() {
            if let Some(event) = self.dispatch() {
                out.push(event);
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f.to_string(), v.strip_prefix(' ').unwrap_or(v).to_string()),
            None => (line, String::new()),
        };
        match field.as_str() {
            "event" => self.event = Some(value),
            "data" => self.data.push(value),
            "id" if !value.contains('\0') => self.id = Some(value),
            "retry" => self.retry = value.parse().ok().or(self.retry),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        let retry = self.retry.take();
        if self.data.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent {
            event: event
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "message".into()),
            data,
            id: self.id.clone(),
            retry,
        })
    }
}

/// Drive a byte stream through the parser, calling `on_event` for each event. Returns when the
/// stream ends or errors (the error text is returned).
pub async fn pump<S, E, F, Fut>(stream: S, mut on_event: F) -> Result<(), String>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Display,
    F: FnMut(SseEvent) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    use futures::StreamExt;
    let mut stream = stream;
    let mut parser = SseParser::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        for event in parser.feed(&chunk) {
            if !on_event(event).await {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_across_chunks_and_line_endings() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: endpoint\r\ndata: /mess").is_empty());
        let events = p.feed(b"age?x=1\r\n\r\n: comment\ndata: a\ndata: b\nid: 7\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "endpoint");
        assert_eq!(events[0].data, "/message?x=1");
        assert_eq!(events[1].event, "message");
        assert_eq!(events[1].data, "a\nb");
        assert_eq!(events[1].id.as_deref(), Some("7"));
    }

    #[test]
    fn empty_data_is_not_dispatched_and_cr_only_works() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: x\n\n").is_empty());
        let events = p.feed(b"data:{}\r\r");
        assert_eq!(events[0].data, "{}");
        assert_eq!(events[0].event, "message");
    }
}
