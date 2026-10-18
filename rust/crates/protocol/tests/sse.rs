//! Purpose: SSE framing parity with Effect's default encoder and decoder edge cases.
//! Scope: pure, fixture-free.
//! Uses:
//! rust/crates/
//! `-- protocol/src/sse.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: encode/decode round-trips multiline data, split CRLF and comments.
//! Contract tests: protocol/tests/sse.rs

use opencode_protocol::sse::{Decoder, Frame};

#[test]
fn encodes_like_effect() {
    assert_eq!(Frame::message("{\"a\":1}").encode(), "data: {\"a\":1}\n\n");
    let f = Frame {
        id: Some("7".into()),
        event: "x".into(),
        data: "a\nb".into(),
    };
    assert_eq!(f.encode(), "id: 7\nevent: x\ndata: a\ndata: b\n\n");
    assert_eq!(Frame::message("").encode(), "\n");
}

#[test]
fn decodes_split_crlf_multiline_and_comments() {
    let mut d = Decoder::default();
    assert!(d.push(": heartbeat\r").is_empty());
    assert!(d.push("\n\r\ndata: a\r").is_empty());
    let frames = d.push("\ndata: b\r\n\r\n");
    assert_eq!(frames, vec![Frame::message("a\nb")]);
}
