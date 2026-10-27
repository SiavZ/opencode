//! Purpose: decode Anthropic Bedrock binary eventstreams into baseline SSE bytes.
//! Scope: incremental AWS eventstream framing, CRC validation and base64 JSON payloads.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! |-- base64::{Engine, engine::general_purpose::STANDARD}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! Invariants: buffers incomplete frames; rejects corrupt or oversized frames without panics.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use super::FormatError;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct BedrockDecoder {
    buffer: Vec<u8>,
}
impl BedrockDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while self.buffer.len() >= 4 {
            let total = u32::from_be_bytes(self.buffer[..4].try_into().unwrap()) as usize;
            if !(16..=16 * 1024 * 1024).contains(&total) {
                return Err(invalid("invalid AWS frame length"));
            }
            if self.buffer.len() < total {
                break;
            }
            let frame = self.buffer.drain(..total).collect::<Vec<_>>();
            let headers_length = u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize;
            if headers_length > total - 16 {
                return Err(invalid("invalid AWS headers length"));
            }
            if crc32(&frame[..8]) != u32::from_be_bytes(frame[8..12].try_into().unwrap())
                || crc32(&frame[..total - 4])
                    != u32::from_be_bytes(frame[total - 4..].try_into().unwrap())
            {
                return Err(invalid("AWS eventstream CRC mismatch"));
            }
            let mut headers = &frame[12..12 + headers_length];
            let mut event = false;
            while !headers.is_empty() {
                let length = usize::from(headers[0]);
                if headers.len() < length + 2 {
                    return Err(invalid("truncated AWS header"));
                }
                let name = &headers[1..1 + length];
                let kind = headers[1 + length];
                headers = &headers[length + 2..];
                let size = match kind {
                    0 | 1 => 0,
                    2 => 1,
                    3 => 2,
                    4 => 4,
                    5 | 8 => 8,
                    9 => 16,
                    6 | 7 => {
                        if headers.len() < 2 {
                            return Err(invalid("truncated AWS header value length"));
                        }
                        let length =
                            usize::from(u16::from_be_bytes(headers[..2].try_into().unwrap()));
                        headers = &headers[2..];
                        length
                    }
                    _ => return Err(invalid("unknown AWS header type")),
                };
                if headers.len() < size {
                    return Err(invalid("truncated AWS header value"));
                }
                if name == b":message-type" && kind == 7 {
                    event = &headers[..size] == b"event";
                }
                headers = &headers[size..];
            }
            if !event {
                continue;
            }
            let envelope: Value = serde_json::from_slice(&frame[12 + headers_length..total - 4])
                .map_err(|error| invalid(&error.to_string()))?;
            let encoded = envelope["bytes"]
                .as_str()
                .ok_or_else(|| invalid("missing Bedrock bytes"))?;
            let decoded = STANDARD
                .decode(encoded)
                .map_err(|error| invalid(&error.to_string()))?;
            let data =
                std::str::from_utf8(&decoded).map_err(|error| invalid(&error.to_string()))?;
            let payload: Value =
                serde_json::from_str(data).map_err(|error| invalid(&error.to_string()))?;
            let event_name = payload["type"]
                .as_str()
                .ok_or_else(|| invalid("missing Anthropic event type"))?;
            out.extend_from_slice(format!("event: {event_name}\ndata: {data}\n\n").as_bytes());
        }
        Ok(out)
    }
    pub fn finish(&mut self) -> Result<(), FormatError> {
        if !self.buffer.is_empty() {
            return Err(invalid("truncated AWS eventstream frame"));
        }
        Ok(())
    }
}
fn invalid(message: &str) -> FormatError {
    FormatError::Stream(message.into())
}
fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, byte| {
        (0..8).fold(crc ^ u32::from(*byte), |crc, _| {
            (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 }
        })
    })
}
