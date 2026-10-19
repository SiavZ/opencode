//! Purpose: text encoding, binary and line-ending detection shared by read/edit/write tools.
//! Binary heuristic is the baseline read tool's (`packages/core/src/tool/read-filesystem.ts:105-152`,
//! `packages/opencode/src/tool/read.ts:182-225`); line endings follow `detectLineEnding` in
//! `packages/core/src/tool/edit.ts:42-45`.
//! Scope: pure byte/string inspection.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/fs.rs
//! External:
//! |-- encoding_rs::UTF_16BE
//! |-- encoding_rs::UTF_16LE
//! |-- u16::to_be_bytes
//! `-- u16::to_le_bytes
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! `-- platform-native/tests/fs_encoding.rs
//! Invariants: a UTF-16 BOM wins over the NUL heuristic (UTF-16 text is full of NULs); any NUL
//! otherwise means binary; more than 30% control bytes (below 9 or 14..=31) means binary; an empty
//! sample is text.
//! Contract tests: platform-native/tests/fs_encoding.rs

use opencode_platform_api::fs::{LineEnding, TextEncoding};
use std::path::Path;

/// Extensions the baseline read tools always treat as binary.
pub const BINARY_EXTENSIONS: &[&str] = &[
    ".zip", ".tar", ".gz", ".exe", ".dll", ".so", ".class", ".jar", ".war", ".7z", ".doc", ".docx",
    ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp", ".bin", ".dat", ".obj", ".o", ".a",
    ".lib", ".wasm", ".pyc", ".pyo",
];

/// `path.extname(p).toLowerCase()` is in [`BINARY_EXTENSIONS`].
pub fn binary_extension(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    // node extname ignores a single leading dot (".bashrc" has no extension).
    let trimmed = name.strip_prefix('.').unwrap_or(&name);
    let Some(index) = trimmed.rfind('.') else {
        return false;
    };
    let ext = trimmed[index..].to_lowercase();
    BINARY_EXTENSIONS.contains(&ext.as_str())
}

/// The read tool's `binary(resource, bytes)`/`isBinaryFile(filepath, bytes)` heuristic.
pub fn is_binary(path: &Path, sample: &[u8]) -> bool {
    binary_extension(path) || binary_bytes(sample)
}

/// Byte-only part of the heuristic.
pub fn binary_bytes(sample: &[u8]) -> bool {
    if sample.is_empty() {
        return false;
    }
    let mut non_printable = 0usize;
    for &byte in sample {
        if byte == 0 {
            return true;
        }
        if byte < 9 || (byte > 13 && byte < 32) {
            non_printable += 1;
        }
    }
    non_printable as f64 / sample.len() as f64 > 0.3
}

/// Classify bytes. BOMs are authoritative, then the binary heuristic, then UTF-8.
pub fn detect_encoding(bytes: &[u8]) -> TextEncoding {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return TextEncoding::Utf8Bom;
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        return TextEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return TextEncoding::Utf16Be;
    }
    if binary_bytes(bytes) {
        return TextEncoding::Binary;
    }
    TextEncoding::Utf8
}

/// Decode bytes per their detected encoding. Binary yields `None`. The BOM is not part of the
/// returned text; malformed sequences become U+FFFD like `TextDecoder`.
pub fn decode(bytes: &[u8]) -> Option<(TextEncoding, String)> {
    let encoding = detect_encoding(bytes);
    let text = match encoding {
        TextEncoding::Binary => return None,
        TextEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        TextEncoding::Utf8Bom => String::from_utf8_lossy(&bytes[3..]).into_owned(),
        TextEncoding::Utf16Le => encoding_rs::UTF_16LE
            .decode_without_bom_handling(&bytes[2..])
            .0
            .into_owned(),
        TextEncoding::Utf16Be => encoding_rs::UTF_16BE
            .decode_without_bom_handling(&bytes[2..])
            .0
            .into_owned(),
    };
    Some((encoding, text))
}

/// Encode text back into `encoding`, re-emitting its BOM. Binary is rejected with `None`.
pub fn encode(text: &str, encoding: TextEncoding) -> Option<Vec<u8>> {
    match encoding {
        TextEncoding::Binary => None,
        TextEncoding::Utf8 => Some(text.as_bytes().to_vec()),
        TextEncoding::Utf8Bom => {
            let mut out = vec![0xef, 0xbb, 0xbf];
            out.extend_from_slice(text.as_bytes());
            Some(out)
        }
        TextEncoding::Utf16Le => {
            let mut out = vec![0xff, 0xfe];
            out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            Some(out)
        }
        TextEncoding::Utf16Be => {
            let mut out = vec![0xfe, 0xff];
            out.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            Some(out)
        }
    }
}

/// `detectLineEnding`: CRLF when the text contains any `\r\n`, else LF.
pub fn detect_line_ending(text: &str) -> LineEnding {
    if text.contains("\r\n") {
        return LineEnding::CrLf;
    }
    LineEnding::Lf
}

/// `normalizeLineEndings`: CRLF to LF.
pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// `convertToLineEnding`.
pub fn convert_line_ending(text: &str, ending: LineEnding) -> String {
    let normalized = normalize_line_endings(text);
    match ending {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
    }
}
