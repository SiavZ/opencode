//! Purpose: inspect a top-level request model and replay an unbuffered request body.
//! Scope: incremental JSON metadata inspection and one-shot provider model replacement.
//! Uses:
//! `-- none
//! External:
//! |-- bytes::Bytes
//! |-- futures::{Stream, StreamExt}
//! `-- serde_json::from_slice
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen.rs
//! Invariants: only the prefix through model is retained; dropping the stream drops upstream ownership.
//! Contract tests: hosted-adapters/tests/console_gateway_stream.rs

use std::pin::Pin;

use bytes::Bytes;
use futures::{Stream, StreamExt};

pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

pub struct PreparedBody {
    pub model: String,
    prefix: Vec<u8>,
    range: std::ops::Range<usize>,
    remaining: BodyStream,
}

impl PreparedBody {
    pub async fn read(mut stream: BodyStream) -> Result<Self, std::io::Error> {
        let mut prefix = Vec::new();
        let mut scanner = Scanner::default();
        while let Some(chunk) = stream.next().await {
            prefix.extend_from_slice(&chunk?);
            if let Some((model, range)) = scanner.scan(&prefix)? {
                return Ok(Self {
                    model,
                    prefix,
                    range,
                    remaining: stream,
                });
            }
        }
        Ok(Self {
            model: String::new(),
            prefix,
            range: 0..0,
            remaining: stream,
        })
    }

    pub fn stream(mut self, model: &str) -> Result<BodyStream, std::io::Error> {
        if self.range.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Missing model field",
            ));
        }
        let replacement = serde_json::to_vec(model).expect("string JSON encoding");
        self.prefix.splice(self.range, replacement);
        Ok(Box::pin(
            futures::stream::once(async { Ok(Bytes::from(self.prefix)) }).chain(self.remaining),
        ))
    }
}

/// Mirrors requestBody.ts's bounded tail injection, including last stream flag wins.
pub fn include_usage(stream: BodyStream) -> BodyStream {
    struct State {
        stream: BodyStream,
        tail: Vec<u8>,
        inspect: Vec<u8>,
        streaming: bool,
        done: bool,
    }
    Box::pin(futures::stream::try_unfold(
        State {
            stream,
            tail: Vec::new(),
            inspect: Vec::new(),
            streaming: false,
            done: false,
        },
        |mut state| async move {
            if state.done {
                return Ok(None);
            }
            loop {
                match state.stream.next().await {
                    Some(chunk) => {
                        let chunk = chunk?;
                        state.inspect.extend_from_slice(&chunk);
                        for (start, _) in state
                            .inspect
                            .windows(8)
                            .enumerate()
                            .filter(|(_, part)| *part == b"\"stream\"")
                        {
                            let rest = &state.inspect[start + 8..];
                            let rest = rest
                                .iter()
                                .position(|v| !v.is_ascii_whitespace())
                                .map(|i| &rest[i..])
                                .unwrap_or_default();
                            if rest.first() != Some(&b':') {
                                continue;
                            }
                            let rest = &rest[1..];
                            let rest = rest
                                .iter()
                                .position(|v| !v.is_ascii_whitespace())
                                .map(|i| &rest[i..])
                                .unwrap_or_default();
                            if rest.starts_with(b"true") {
                                state.streaming = true;
                            }
                            if rest.starts_with(b"false") {
                                state.streaming = false;
                            }
                        }
                        if state.inspect.len() > 64 {
                            state.inspect.drain(..state.inspect.len() - 64);
                        }
                        state.tail.extend_from_slice(&chunk);
                        if state.tail.len() <= 4096 {
                            continue;
                        }
                        let emitted = state
                            .tail
                            .drain(..state.tail.len() - 4096)
                            .collect::<Vec<_>>();
                        return Ok(Some((Bytes::from(emitted), state)));
                    }
                    None => {
                        state.done = true;
                        if state.streaming {
                            let close = state
                                .tail
                                .iter()
                                .rposition(|byte| *byte == b'}')
                                .ok_or_else(|| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "Invalid JSON request body",
                                    )
                                })?;
                            state.tail.splice(
                                close..=close,
                                b",\"stream_options\":{\"include_usage\":true}}"
                                    .iter()
                                    .copied(),
                            );
                        }
                        let tail = std::mem::take(&mut state.tail);
                        return Ok(Some((Bytes::from(tail), state)));
                    }
                }
            }
        },
    ))
}

#[derive(Default)]
struct Scanner {
    offset: usize,
    depth: usize,
    start: Option<usize>,
    escaped: bool,
    key: Option<String>,
    value: bool,
}

impl Scanner {
    fn scan(
        &mut self,
        bytes: &[u8],
    ) -> Result<Option<(String, std::ops::Range<usize>)>, std::io::Error> {
        while self.offset < bytes.len() {
            let index = self.offset;
            self.offset += 1;
            let byte = bytes[index];
            if let Some(start) = self.start {
                if self.escaped {
                    self.escaped = false;
                    continue;
                }
                if byte == b'\\' {
                    self.escaped = true;
                    continue;
                }
                if byte != b'"' {
                    continue;
                }
                self.start = None;
                if self.depth != 1 {
                    continue;
                }
                let text: String =
                    serde_json::from_slice(&bytes[start..=index]).map_err(std::io::Error::other)?;
                if self.value {
                    self.value = false;
                    if self.key.as_deref() == Some("model") {
                        return Ok(Some((text, start..index + 1)));
                    }
                } else if self.key.is_none() {
                    self.key = Some(text);
                }
                continue;
            }
            match byte {
                b'"' => self.start = Some(index),
                b'{' | b'[' => {
                    self.depth += 1;
                    if self.depth == 2 {
                        self.value = false;
                    }
                }
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                b':' if self.depth == 1 => self.value = true,
                b',' if self.depth == 1 => {
                    self.key = None;
                    self.value = false;
                }
                byte if self.depth == 1 && self.value && !byte.is_ascii_whitespace() => {
                    self.value = false
                }
                _ => {}
            }
        }
        Ok(None)
    }
}
