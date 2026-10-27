//! Purpose: verify provider SSE byte transport supports all active baseline delimiters.
//! Scope: public StreamConverter LF, CRLF, CR-only and byte-fragmented frame boundaries.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: delimiter choice never changes normalized converted SSE part output.
//! Contract tests: hosted-adapters/tests/console_formats_transport.rs

use opencode_hosted_adapters::console::formats::{Format, StreamConverter, convert_stream_part};

#[test]
fn all_baseline_sse_delimiters_and_frame_whitespace() {
    let part = "data: {\"choices\":[{\"delta\":{\"content\":\"text\"}}]}";
    let expected = convert_stream_part(part, Format::OaCompat, Format::Anthropic).unwrap();
    for delimiter in ["\n\n", "\r\n\r\n", "\r\r"] {
        let bytes = format!("  {part}  {delimiter}");
        let mut converter = StreamConverter::new(Format::OaCompat, Format::Anthropic);
        let mut out = Vec::new();
        for byte in bytes.bytes() {
            out.extend(converter.push(&[byte]).unwrap());
        }
        out.extend(converter.finish().unwrap());
        assert_eq!(String::from_utf8(out).unwrap(), format!("{expected}\n\n"));
    }
}
