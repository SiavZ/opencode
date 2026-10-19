//! Purpose: encoding, binary and line-ending detection tests mirroring the baseline read/edit
//! heuristics, including UTF-16 round trips and real-file BOM sniffing.
//! Scope: pure tests plus one tempfile read.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! `-- platform-native/src/fs/encoding.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: none beyond test isolation.
//! Contract tests: platform-native/tests/fs_encoding.rs

use opencode_platform_api::fs::{LineEnding, TextEncoding};
use opencode_platform_native::fs::encoding::{
    binary_bytes, binary_extension, convert_line_ending, decode, detect_encoding,
    detect_line_ending, encode, is_binary,
};
use std::path::Path;

#[test]
fn binary_heuristic_matches_the_read_tool() {
    assert!(!binary_bytes(b""));
    assert!(binary_bytes(b"abc\0def"));
    assert!(!binary_bytes(b"line\r\n\tok\x0b\x0c"));
    // 3 of 10 control bytes is exactly 30%: not binary (strictly greater required).
    assert!(!binary_bytes(b"\x01\x02\x03abcdefg"));
    assert!(binary_bytes(b"\x01\x02\x03\x04abcdef"));
    assert!(is_binary(Path::new("/x/archive.ZIP"), b"plain"));
    assert!(binary_extension(Path::new("lib.so")));
    assert!(
        !binary_extension(Path::new(".bin")),
        "node extname of a dotfile is empty"
    );
    assert!(!binary_extension(Path::new("notes.txt")));
}

#[test]
fn detects_boms_before_the_nul_heuristic() {
    assert_eq!(detect_encoding(b"hello"), TextEncoding::Utf8);
    assert_eq!(detect_encoding(b"\xef\xbb\xbfhi"), TextEncoding::Utf8Bom);
    assert_eq!(detect_encoding(b"\xff\xfeh\0i\0"), TextEncoding::Utf16Le);
    assert_eq!(detect_encoding(b"\xfe\xff\0h\0i"), TextEncoding::Utf16Be);
    assert_eq!(detect_encoding(b"h\0i\0"), TextEncoding::Binary);
}

#[test]
fn decode_and_encode_round_trip_every_text_encoding() {
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let text = "héllo\r\nwörld 😀";
        let bytes = encode(text, encoding).unwrap();
        assert_eq!(
            decode(&bytes),
            Some((encoding, text.to_string())),
            "{encoding:?}"
        );
    }
    assert_eq!(encode("x", TextEncoding::Binary), None);
    assert_eq!(decode(b"\0\x01"), None);
    assert_eq!(decode(b"a\xffb").unwrap().1, "a\u{fffd}b");
}

#[test]
fn line_endings_follow_detect_line_ending() {
    assert_eq!(detect_line_ending("a\nb"), LineEnding::Lf);
    assert_eq!(detect_line_ending("a\nb\r\nc"), LineEnding::CrLf);
    assert_eq!(detect_line_ending("a\rb"), LineEnding::Lf);
    assert_eq!(
        convert_line_ending("a\r\nb\nc", LineEnding::CrLf),
        "a\r\nb\r\nc"
    );
    assert_eq!(convert_line_ending("a\r\nb\nc", LineEnding::Lf), "a\nb\nc");
}

#[test]
fn sniffs_real_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf16.txt");
    std::fs::write(&path, encode("data", TextEncoding::Utf16Le).unwrap()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(detect_encoding(&bytes), TextEncoding::Utf16Le);
    assert!(
        is_binary(&path, &bytes),
        "the read tool still rejects UTF-16 (NUL bytes)"
    );
}
