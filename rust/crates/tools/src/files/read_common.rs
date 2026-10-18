//! Purpose: byte and text primitives shared by both read-tool generations (binary sniffing, MIME
//! lookup, file URLs, JS string semantics, streaming UTF-8, `localeCompare` ordering).
//! Scope: pure helpers, no filesystem access.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_files.rs
//! |-- tools/src/files/read_input.rs
//! |-- tools/src/files/read_legacy.rs
//! |-- tools/src/files/read_lines.rs
//! `-- tools/src/files/read_pager.rs
//! Invariants: MIME table equals mime-types 3.0.2 `types` (fixtures/README.md), line lengths count
//! UTF-16 code units like JS `String.length`, UTF-8 replacement follows WHATWG maximal subparts.
//! Contract tests: tools/tests/files_read.rs

use serde_json::Value;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const MAX_READ_LINES: usize = 2_000;
pub const MAX_READ_BYTES: usize = 50 * 1024;
pub const MAX_MEDIA_INGEST_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_LINE_LENGTH: usize = 2_000;
pub const CHUNK_BYTES: usize = 64 * 1024;
pub const SUPPORTED_IMAGE_MIMES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

pub fn line_suffix() -> String {
    format!("... (line truncated to {MAX_LINE_LENGTH} chars)")
}

const BINARY_EXTENSIONS: [&str; 28] = [
    ".zip", ".tar", ".gz", ".exe", ".dll", ".so", ".class", ".jar", ".war", ".7z", ".doc", ".docx",
    ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp", ".bin", ".dat", ".obj", ".o", ".a",
    ".lib", ".wasm", ".pyc", ".pyo",
];

/// Node `path.extname` for `/`-separated (or native) paths.
pub fn extname(path: &str) -> &str {
    let base = path
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(path);
    match base.rfind('.') {
        Some(0) | None => "",
        Some(_) if base.bytes().all(|byte| byte == b'.') => "",
        Some(index) => &base[index..],
    }
}

pub fn binary_extension(path: &str) -> bool {
    BINARY_EXTENSIONS.contains(&extname(path).to_lowercase().as_str())
}

/// Shared binary heuristic: known extension, any NUL, or more than 30% control bytes.
pub fn is_binary(path: &str, bytes: &[u8]) -> bool {
    if binary_extension(path) {
        return true;
    }
    if bytes.is_empty() {
        return false;
    }
    let mut non_printable = 0usize;
    for &byte in bytes {
        if byte == 0 {
            return true;
        }
        if byte < 9 || (byte > 13 && byte < 32) {
            non_printable += 1;
        }
    }
    non_printable as f64 / bytes.len() as f64 > 0.3
}

/// Core `imageMime`: supported image magic only.
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        return Some("image/webp");
    }
    None
}

/// Legacy `sniffAttachmentMime` (packages/opencode/src/util/media.ts).
pub fn sniff_attachment_mime(bytes: &[u8], fallback: String) -> String {
    if bytes.starts_with(&[0x42, 0x4d]) && image_mime(bytes).is_none() {
        return "image/bmp".into();
    }
    if bytes.starts_with(b"%PDF-") {
        return "application/pdf".into();
    }
    image_mime(bytes).map(Into::into).unwrap_or(fallback)
}

/// `FSUtil.mimeType`: mime-types `lookup(path) || "application/octet-stream"`.
pub fn mime_type(path: &str) -> String {
    static TYPES: OnceLock<HashMap<String, String>> = OnceLock::new();
    let types = TYPES.get_or_init(|| {
        serde_json::from_str(include_str!("fixtures/mime-types.json"))
            .expect("validated bundled MIME fixture")
    });
    let extension = extname(&format!("x.{path}")).to_lowercase();
    extension
        .get(1..)
        .filter(|extension| !extension.is_empty())
        .and_then(|extension| types.get(extension))
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".into())
}

/// Node `url.pathToFileURL(path).href` for absolute POSIX paths.
pub fn file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for byte in slash(path).bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-._/;=@:,!$&'()*+".contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn slash(path: &str) -> String {
    path.replace('\\', "/")
}

/// JS `String.length`.
pub fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// JS `text.slice(0, max)`. A surrogate pair cut in half becomes U+FFFD, which has the same
/// UTF-8 byte length as the lone surrogate `Buffer.byteLength` would count.
pub fn js_slice(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for character in text.chars() {
        let width = character.len_utf16();
        if units + width > max {
            if units < max {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(character);
        units += width;
    }
    out
}

/// Line truncation shared by both generations.
pub fn truncate_line(text: &str) -> String {
    if js_len(text) > MAX_LINE_LENGTH {
        return js_slice(text, MAX_LINE_LENGTH) + &line_suffix();
    }
    text.into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed UTF-8")]
pub struct Malformed;

/// Streaming `TextDecoder("utf-8", { fatal })` with BOM stripping (`ignoreBOM: false`).
#[derive(Debug, Default)]
pub struct Utf8Stream {
    carry: Vec<u8>,
    started: bool,
    fatal: bool,
}

impl Utf8Stream {
    pub fn new(fatal: bool) -> Self {
        Self {
            fatal,
            ..Self::default()
        }
    }

    /// `decoder.decode(bytes, { stream: true })`.
    pub fn push(&mut self, bytes: &[u8]) -> Result<String, Malformed> {
        let mut buffer = std::mem::take(&mut self.carry);
        buffer.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest = buffer.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let (valid, tail) = rest.split_at(error.valid_up_to());
                    out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    let Some(length) = error.error_len() else {
                        self.carry = tail.to_vec();
                        break;
                    };
                    if self.fatal {
                        return Err(Malformed);
                    }
                    out.push('\u{FFFD}');
                    rest = &tail[length..];
                }
            }
        }
        Ok(self.strip_bom(out))
    }

    /// `decoder.decode()`: flush an incomplete trailing sequence.
    pub fn finish(&mut self) -> Result<String, Malformed> {
        if self.carry.is_empty() {
            return Ok(String::new());
        }
        self.carry.clear();
        if self.fatal {
            return Err(Malformed);
        }
        Ok(self.strip_bom("\u{FFFD}".into()))
    }

    fn strip_bom(&mut self, text: String) -> String {
        if self.started || text.is_empty() {
            return text;
        }
        self.started = true;
        text.strip_prefix('\u{FEFF}')
            .map(Into::into)
            .unwrap_or(text)
    }
}

/// Printable ASCII ordered like `localeCompare` under the baseline Bun ICU root collation,
/// letters share a primary weight with lowercase first.
const ASCII_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";

fn weight(character: char) -> (u32, u8) {
    if character.is_ascii_alphabetic() {
        let index = ASCII_ORDER
            .find(character.to_ascii_lowercase())
            .unwrap_or(0) as u32;
        return (index, u8::from(character.is_ascii_uppercase()));
    }
    match ASCII_ORDER.find(character) {
        Some(index) => (index as u32, 0),
        None => (1000 + character as u32, 0),
    }
}

/// `a.localeCompare(b)` for ASCII names. Non-ASCII characters sort after ASCII by code point,
/// an approximation of ICU (recorded in phase-05 status).
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let weights = |text: &str| -> Vec<(u32, u8)> {
        text.chars()
            .filter(|character| *character as u32 >= 0x20)
            .map(weight)
            .collect()
    };
    let (left, right) = (weights(a), weights(b));
    left.iter()
        .map(|w| w.0)
        .cmp(right.iter().map(|w| w.0))
        .then_with(|| left.iter().map(|w| w.1).cmp(right.iter().map(|w| w.1)))
}

/// Effect `formatUnknown` for issue messages: JSON with JS number formatting.
pub fn format_unknown(value: &Value) -> String {
    match value {
        Value::Number(number) => js_number(number.as_f64().unwrap_or_default()),
        other => other.to_string(),
    }
}

/// JS `Number.prototype.toString`.
pub fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{value:.0}");
    }
    if value.abs() >= 1e21 || value.abs() < 1e-6 {
        let text = format!("{value:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => text,
        };
    }
    value.to_string()
}
