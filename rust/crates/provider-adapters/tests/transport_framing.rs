//! Purpose: split-chunk fixtures for SSE and AWS event-stream framing: every byte boundary must
//! yield the same frames as a single chunk; malformed frames fail with smithy-equivalent text.
//! Scope: pure decoder tests, no I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/transport/event_stream.rs
//! `-- provider-adapters/src/transport/sse.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture frames are built with `encode_frame` and checked against the CRC32
//! standard check value, so the encoder is not trusted blindly.
//! Contract tests: provider-adapters/tests/transport_framing.rs

use opencode_llm_api::LlmErrorReason;
use opencode_provider_adapters::transport::event_stream::{
    EventStreamDecoder, crc32, encode_frame,
};
use opencode_provider_adapters::transport::sse::{SseError, SseParser};
use serde_json::{Value, json};

fn stream_bytes() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(encode_frame(
        &[(":message-type", "event"), (":event-type", "messageStart")],
        br#"{"role":"assistant","p":"abc"}"#,
    ));
    body.extend(encode_frame(
        &[
            (":message-type", "event"),
            (":event-type", "contentBlockDelta"),
        ],
        "{\"delta\":{\"text\":\"h\u{e9}llo\"}}".as_bytes(),
    ));
    body.extend(encode_frame(
        &[(":message-type", "event"), (":event-type", "metadata")],
        br#"{"usage":{"inputTokens":3},"p":"x"}"#,
    ));
    body
}

fn expected() -> Vec<Value> {
    vec![
        json!({"messageStart": {"role": "assistant"}}),
        json!({"contentBlockDelta": {"delta": {"text": "h\u{e9}llo"}}}),
        json!({"metadata": {"usage": {"inputTokens": 3}}}),
    ]
}

#[test]
fn crc32_matches_ieee_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    let frame = encode_frame(&[], b"");
    assert_eq!(frame.len(), 16);
    // Known-good empty AWS event-stream message (total 16, headers 0).
    assert_eq!(
        frame,
        [
            0, 0, 0, 16, 0, 0, 0, 0, 0x05, 0xc2, 0x48, 0xeb, 0x7d, 0x98, 0xc8, 0xff
        ]
    );
}

#[test]
fn event_stream_every_split_point() {
    let bytes = stream_bytes();
    for split in 0..=bytes.len() {
        let mut decoder = EventStreamDecoder::new("bedrock-converse");
        let mut out = decoder.feed(&bytes[..split]).unwrap();
        out.extend(decoder.feed(&bytes[split..]).unwrap());
        assert_eq!(out, expected(), "split at {split}");
    }
    let mut decoder = EventStreamDecoder::new("bedrock-converse");
    let mut out = Vec::new();
    for byte in &bytes {
        out.extend(decoder.feed(std::slice::from_ref(byte)).unwrap());
    }
    assert_eq!(out, expected());
}

fn decode_err(bytes: &[u8]) -> String {
    let error = EventStreamDecoder::new("bedrock-converse")
        .feed(bytes)
        .unwrap_err();
    let LlmErrorReason::InvalidProviderOutput { route, message, .. } = error.reason.as_ref() else {
        panic!("{error:?}");
    };
    assert_eq!(route.as_deref(), Some("bedrock-converse"));
    message.clone()
}

#[test]
fn event_stream_malformed_frames() {
    let good = encode_frame(&[(":message-type", "event"), (":event-type", "x")], b"{}");
    let mut bad_prelude = good.clone();
    bad_prelude[8] ^= 1;
    assert!(
        decode_err(&bad_prelude).starts_with(
            "Failed to decode Bedrock Converse event-stream frame: The prelude checksum specified in the message ("
        )
    );
    let mut bad_payload = good.clone();
    bad_payload[good.len() - 6] ^= 1;
    assert!(decode_err(&bad_payload).contains("The message checksum ("));
    assert_eq!(
        decode_err(&[0, 0, 0, 8, 0, 0, 0, 0]),
        "Failed to decode Bedrock Converse event-stream frame: Provided message too short to accommodate event stream message overhead"
    );
    let mut bad_tag = encode_frame(&[(":message-type", "event")], b"{}");
    // Header value type byte lives after [len=1]["name"(13)] at offset 12 + 1 + 13.
    bad_tag[12 + 1 + 13] = 42;
    let total = bad_tag.len();
    let prelude_crc = crc32(&bad_tag[..8]).to_be_bytes();
    bad_tag[8..12].copy_from_slice(&prelude_crc);
    let message_crc = crc32(&bad_tag[..total - 4]).to_be_bytes();
    bad_tag[total - 4..].copy_from_slice(&message_crc);
    assert!(decode_err(&bad_tag).ends_with("Unrecognized header type tag"));

    let bad_json = encode_frame(
        &[(":message-type", "event"), (":event-type", "x")],
        b"{nope",
    );
    let error = EventStreamDecoder::new("bedrock-converse")
        .feed(&bad_json)
        .unwrap_err();
    let LlmErrorReason::InvalidProviderOutput { message, raw, .. } = error.reason.as_ref() else {
        panic!();
    };
    assert_eq!(
        message,
        "Failed to parse Bedrock Converse event-stream payload"
    );
    assert_eq!(raw.as_deref(), Some("{nope"));
}

#[test]
fn event_stream_frame_limit_is_enforced_before_buffering() {
    let mut decoder = EventStreamDecoder::with_limit("bedrock-converse", 64);
    let error = decoder.feed(&[0, 0, 1, 0]).unwrap_err();
    assert!(error.reason.message().contains("exceeds the 64 byte limit"));
}

#[test]
fn event_stream_non_string_message_type_is_skipped() {
    // `:message-type` encoded as a boolean header (tag 0) must not match "event".
    let mut headers = vec![13u8];
    headers.extend_from_slice(b":message-type");
    headers.push(0);
    let total = 16 + headers.len() + 2;
    let mut frame = Vec::new();
    frame.extend_from_slice(&(total as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(b"{}");
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    let out = EventStreamDecoder::new("r").feed(&frame).unwrap();
    assert!(out.is_empty());
}

#[test]
fn sse_every_split_point() {
    let body = "\u{feff}data: {\"a\":\"h\u{e9}\"}\r\n\r\nevent: x\nid: 1\ndata: l1\ndata: l2\n\n: c\n\ndata: [DONE]\n\n";
    let bytes = body.as_bytes();
    let want = ["{\"a\":\"h\u{e9}\"}", "l1\nl2"];
    for split in 0..=bytes.len() {
        let mut parser = SseParser::default();
        let mut out = parser.feed(&bytes[..split]).unwrap();
        out.extend(parser.feed(&bytes[split..]).unwrap());
        assert_eq!(out, want, "split at {split}");
    }
}

#[test]
fn sse_trailing_event_without_blank_line_is_dropped() {
    let mut parser = SseParser::default();
    assert!(parser.feed(b"data: tail").unwrap().is_empty());
}

#[test]
fn sse_bounded_event() {
    let mut parser = SseParser::new(32);
    let mut result = Ok(Vec::new());
    for _ in 0..10 {
        result = parser.feed(b"data: 0123456789\n");
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result, Err(SseError::TooLarge(32)));
}
