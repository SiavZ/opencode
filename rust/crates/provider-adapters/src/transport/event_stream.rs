//! Purpose: AWS event-stream binary framing for Bedrock Converse
//! (packages/llm/src/protocols/bedrock-event-stream.ts plus @smithy/eventstream-codec
//! `splitMessage`/`HeaderMarshaller.parse`). Bytes in, `{ "<:event-type>": payload }` JSON out.
//! Scope: pure incremental decoder, no I/O.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/http.rs
//! `-- provider-adapters/src/transport/mod.rs
//! Invariants: frame = `[total:4][headers:4][prelude crc:4][headers][payload][message crc:4]`,
//! big-endian. Prelude and message CRC32 (IEEE) are verified before headers are parsed. Only
//! `:message-type == "event"` frames with a string `:event-type` and a non-empty payload are
//! emitted; a top-level `p` padding key is removed. A frame larger than `max_frame_bytes` fails
//! instead of buffering. Error messages mirror the smithy codec text.
//! Contract tests: provider-adapters/tests/transport_framing.rs

use opencode_llm_api::LlmError;
use serde_json::Value;

const PRELUDE_LENGTH: usize = 8;
const CHECKSUM_LENGTH: usize = 4;
const MINIMUM_MESSAGE_LENGTH: usize = PRELUDE_LENGTH + CHECKSUM_LENGTH * 2;
/// Default bound on one event-stream frame (16 MiB, the AWS maximum).
pub const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320), as used by AWS event streams.
pub fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    });
    let mut crc = !0u32;
    for &b in bytes {
        crc = TABLE[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Decoded header value; only strings matter for routing, others are kept for completeness.
#[derive(Clone, Debug, PartialEq)]
pub enum HeaderValue {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Binary(Vec<u8>),
    String(String),
    Timestamp(i64),
    Uuid(String),
}

/// Incremental decoder. `route` labels errors (`bedrock-converse` in the baseline).
#[derive(Debug)]
pub struct EventStreamDecoder {
    route: String,
    buffer: Vec<u8>,
    max_frame_bytes: usize,
}

impl EventStreamDecoder {
    pub fn new(route: impl Into<String>) -> Self {
        Self::with_limit(route, DEFAULT_MAX_FRAME_BYTES)
    }

    pub fn with_limit(route: impl Into<String>, max_frame_bytes: usize) -> Self {
        Self {
            route: route.into(),
            buffer: Vec::new(),
            max_frame_bytes,
        }
    }

    /// Feed a network chunk, returning every projected event completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Value>, LlmError> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut offset = 0;
        while self.buffer.len() - offset >= 4 {
            let view = &self.buffer[offset..];
            let total = u32::from_be_bytes([view[0], view[1], view[2], view[3]]) as usize;
            if total > self.max_frame_bytes {
                return Err(self.decode_error(&format!(
                    "Event stream frame of {total} bytes exceeds the {} byte limit",
                    self.max_frame_bytes
                )));
            }
            if view.len() < total {
                break;
            }
            // A reported length below the 16-byte minimum fails in `split_message`, so the
            // loop always advances or returns.
            let frame = &view[..total];
            offset += total;
            if let Some(event) = self.project(frame)? {
                out.push(event);
            }
        }
        self.buffer.drain(..offset);
        Ok(out)
    }

    fn project(&self, frame: &[u8]) -> Result<Option<Value>, LlmError> {
        let (headers, body) = split_message(frame).map_err(|e| self.decode_error(&e))?;
        let headers = parse_headers(headers).map_err(|e| self.decode_error(&e))?;
        let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v);
        if header(":message-type") != Some(&HeaderValue::String("event".into())) {
            return Ok(None);
        }
        let Some(HeaderValue::String(event_type)) = header(":event-type") else {
            return Ok(None);
        };
        let payload = String::from_utf8_lossy(body);
        if payload.is_empty() {
            return Ok(None);
        }
        let mut parsed: Value = serde_json::from_str(&payload).map_err(|_| {
            LlmError::invalid_output(
                &self.route,
                "Failed to parse Bedrock Converse event-stream payload",
                Some(payload.clone().into_owned()),
            )
        })?;
        if let Value::Object(map) = &mut parsed {
            map.shift_remove("p");
        }
        let mut wrapped = serde_json::Map::new();
        wrapped.insert(event_type.clone(), parsed);
        Ok(Some(Value::Object(wrapped)))
    }

    fn decode_error(&self, message: &str) -> LlmError {
        LlmError::invalid_output(
            &self.route,
            format!("Failed to decode Bedrock Converse event-stream frame: {message}"),
            None,
        )
    }
}

/// smithy `splitMessage`: validate lengths and CRCs, return (headers, body).
fn split_message(frame: &[u8]) -> Result<(&[u8], &[u8]), String> {
    if frame.len() < MINIMUM_MESSAGE_LENGTH {
        return Err(
            "Provided message too short to accommodate event stream message overhead".into(),
        );
    }
    let be =
        |at: usize| u32::from_be_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]);
    if frame.len() != be(0) as usize {
        return Err("Reported message length does not match received message length".into());
    }
    let header_length = be(4) as usize;
    let expected_prelude = be(PRELUDE_LENGTH);
    let expected_message = be(frame.len() - CHECKSUM_LENGTH);
    let prelude = crc32(&frame[..PRELUDE_LENGTH]);
    if expected_prelude != prelude {
        return Err(format!(
            "The prelude checksum specified in the message ({expected_prelude}) does not match the calculated CRC32 checksum ({prelude})"
        ));
    }
    let message = crc32(&frame[..frame.len() - CHECKSUM_LENGTH]);
    if expected_message != message {
        return Err(format!(
            "The message checksum ({message}) did not match the expected value of {expected_message}"
        ));
    }
    let headers_start = PRELUDE_LENGTH + CHECKSUM_LENGTH;
    let body_end = frame.len() - CHECKSUM_LENGTH;
    let headers_end = headers_start
        .checked_add(header_length)
        .filter(|end| *end <= body_end)
        .ok_or_else(|| "Offset is outside the bounds of the DataView".to_owned())?;
    Ok((
        &frame[headers_start..headers_end],
        &frame[headers_end..body_end],
    ))
}

/// smithy `HeaderMarshaller.parse`.
fn parse_headers(bytes: &[u8]) -> Result<Vec<(String, HeaderValue)>, String> {
    let oob = || "Offset is outside the bounds of the DataView".to_owned();
    let take = |pos: &mut usize, n: usize| -> Result<&[u8], String> {
        let slice = bytes.get(*pos..*pos + n).ok_or_else(oob)?;
        *pos += n;
        Ok(slice)
    };
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let name_len = take(&mut pos, 1)?[0] as usize;
        let name = String::from_utf8_lossy(take(&mut pos, name_len)?).into_owned();
        let tag = take(&mut pos, 1)?[0];
        let value = match tag {
            0 => HeaderValue::Bool(true),
            1 => HeaderValue::Bool(false),
            2 => HeaderValue::Byte(take(&mut pos, 1)?[0] as i8),
            3 => HeaderValue::Short(i16::from_be_bytes(
                take(&mut pos, 2)?.try_into().map_err(|_| oob())?,
            )),
            4 => HeaderValue::Int(i32::from_be_bytes(
                take(&mut pos, 4)?.try_into().map_err(|_| oob())?,
            )),
            5 | 8 => {
                let v = i64::from_be_bytes(take(&mut pos, 8)?.try_into().map_err(|_| oob())?);
                if tag == 5 {
                    HeaderValue::Long(v)
                } else {
                    HeaderValue::Timestamp(v)
                }
            }
            6 | 7 => {
                let len =
                    u16::from_be_bytes(take(&mut pos, 2)?.try_into().map_err(|_| oob())?) as usize;
                let raw = take(&mut pos, len)?;
                if tag == 6 {
                    HeaderValue::Binary(raw.to_vec())
                } else {
                    HeaderValue::String(String::from_utf8_lossy(raw).into_owned())
                }
            }
            9 => {
                let hex = hex::encode(take(&mut pos, 16)?);
                HeaderValue::Uuid(format!(
                    "{}-{}-{}-{}-{}",
                    &hex[..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..]
                ))
            }
            _ => return Err("Unrecognized header type tag".into()),
        };
        out.push((name, value));
    }
    Ok(out)
}

/// Encode one frame with string headers (fixtures and tests; mirrors the smithy encoder).
pub fn encode_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut header_bytes = Vec::new();
    for (name, value) in headers {
        header_bytes.push(name.len() as u8);
        header_bytes.extend_from_slice(name.as_bytes());
        header_bytes.push(7);
        header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        header_bytes.extend_from_slice(value.as_bytes());
    }
    let total = MINIMUM_MESSAGE_LENGTH + header_bytes.len() + payload.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32(&out).to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&out).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
