//! Purpose: incremental Server-Sent Events framing: bytes in, `data:` payload strings out,
//! dropping empty and `[DONE]` keep-alives (`ProviderShared.sseFraming`).
//! Scope: pure incremental parser, no I/O.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/http.rs
//! `-- provider-adapters/src/transport/mod.rs
//! Invariants: line handling matches effect/unstable/encoding/Sse.ts `makeParser`: CR, LF
//! and CRLF terminate lines (CRLF split across chunks is handled), BOM stripped once,
//! `data` lines join with `\n`, an event dispatches on a blank line only when data is
//! non-empty, a trailing event without a blank line is not dispatched. A numeric `retry:` field
//! ends the stream after the events of the chunk that carried it (the baseline maps the
//! `Sse.Retry` failure to `Stream.empty`). Multi-byte UTF-8 split across chunks is reassembled,
//! invalid sequences become U+FFFD (`TextDecoder` replacement mode). Buffered bytes are
//! bounded by `max_event_bytes`.
//! Contract tests: provider-adapters/src/transport/sse.rs (unit tests)

/// Default bound on one buffered SSE event (16 MiB).
pub const DEFAULT_MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct SseParser {
    pending: Vec<u8>,
    line: String,
    data: String,
    has_data: bool,
    first: bool,
    skip_lf: bool,
    retry: bool,
    max_event_bytes: usize,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SseError {
    #[error("SSE event exceeded {0} bytes")]
    TooLarge(usize),
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_EVENT_BYTES)
    }
}

impl SseParser {
    pub fn new(max_event_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            line: String::new(),
            data: String::new(),
            has_data: false,
            first: true,
            skip_lf: false,
            retry: false,
            max_event_bytes,
        }
    }

    /// True once a numeric `retry:` field was parsed: the caller must stop reading.
    pub fn ended(&self) -> bool {
        self.retry
    }

    /// Feed a chunk and return the payloads of completed, non-empty, non-`[DONE]` events.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<String>, SseError> {
        self.pending.extend_from_slice(chunk);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    text.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid = err.valid_up_to();
                    text.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    let Some(bad) = err.error_len() else {
                        self.pending.drain(..valid);
                        break;
                    };
                    text.push('\u{fffd}');
                    self.pending.drain(..valid + bad);
                }
            }
        }
        let mut text = text.as_str();
        if self.first && !text.is_empty() {
            self.first = false;
            text = text.strip_prefix('\u{feff}').unwrap_or(text);
        }
        let mut out = Vec::new();
        for ch in text.chars() {
            if self.skip_lf {
                self.skip_lf = false;
                if ch == '\n' {
                    continue;
                }
            }
            match ch {
                '\r' | '\n' => {
                    self.skip_lf = ch == '\r';
                    let line = std::mem::take(&mut self.line);
                    if let Some(event) = self.line_done(&line) {
                        out.push(event);
                    }
                }
                _ => self.line.push(ch),
            }
            if self.line.len() + self.data.len() > self.max_event_bytes {
                return Err(SseError::TooLarge(self.max_event_bytes));
            }
        }
        Ok(out)
    }

    fn line_done(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            let has = std::mem::take(&mut self.has_data);
            let mut data = std::mem::take(&mut self.data);
            if !has {
                return None;
            }
            data.pop();
            return (!data.is_empty() && data != "[DONE]").then_some(data);
        }
        let (field, value) = match line.find(':') {
            Some(idx) => {
                let value = &line[idx + 1..];
                (&line[..idx], value.strip_prefix(' ').unwrap_or(value))
            }
            None => (line, ""),
        };
        if field == "data" {
            self.data.push_str(value);
            self.data.push('\n');
            self.has_data = true;
        } else if field == "retry" && js_parse_int(value) {
            self.retry = true;
        }
        None
    }
}

/// JS `!Number.isNaN(parseInt(value, 10))`: optional whitespace and sign, then a digit.
fn js_parse_int(value: &str) -> bool {
    let text = value.trim_start();
    let text = text.strip_prefix(['+', '-']).unwrap_or(text);
    text.starts_with(|c: char| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_crlf_bom_multiline_and_done() {
        let mut p = SseParser::default();
        let mut out = p.feed(b"\xef\xbb\xbfdata: a\r").unwrap();
        out.extend(
            p.feed(b"\ndata: b\r\n\r\ndata: [DONE]\n\n: comment\ndata:\n\n")
                .unwrap(),
        );
        assert_eq!(out, ["a\nb"]);
        let mut p = SseParser::new(8);
        assert_eq!(p.feed(b"data: 0123456789"), Err(SseError::TooLarge(8)));
    }

    #[test]
    fn multibyte_split_across_chunks() {
        let mut p = SseParser::default();
        let bytes = "data: h\u{e9}\n\n".as_bytes();
        let mut out = p.feed(&bytes[..8]).unwrap();
        out.extend(p.feed(&bytes[8..]).unwrap());
        assert_eq!(out, ["h\u{e9}"]);
    }

    #[test]
    fn retry_ends_stream_after_its_chunk_and_invalid_utf8_is_replaced() {
        let mut p = SseParser::default();
        let out = p.feed(b"data: a\n\nretry: 10\ndata: b\n\n").unwrap();
        assert_eq!(out, ["a", "b"]);
        assert!(p.ended());
        let mut p = SseParser::default();
        assert!(p.feed(b"retry: soon\n\n").unwrap().is_empty());
        assert!(!p.ended());
        assert_eq!(p.feed(b"data: \xffx\n\n").unwrap(), ["\u{fffd}x"]);
    }
}
