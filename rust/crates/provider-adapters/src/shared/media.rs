//! Purpose: validate and normalize provider media payloads.
//! Scope: pure media MIME, size, data URL and canonical base64 validation.
//! Uses: opencode_llm_api::{LlmError, MediaData}, base64::{Engine, STANDARD}.
//! Used by: provider-adapters/src/shared.rs through unchanged public reexports.
//! Invariants: preserves validation order, limits, error messages and canonical encoding.
//! Contract tests: provider-adapters/src/shared/media.rs and provider integration tests.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use opencode_llm_api::{LlmError, MediaData};

pub const IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
pub const VIDEO_MIMES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];
pub const AUDIO_MIMES: &[&str] = &[
    "audio/wav",
    "audio/mp3",
    "audio/aiff",
    "audio/aac",
    "audio/ogg",
    "audio/flac",
];
pub const MAX_MEDIA_ENCODED_BYTES: usize = 28 * 1024 * 1024;
pub const MAX_MEDIA_DECODED_BYTES: usize = 20 * 1024 * 1024;

/// Validated media payload.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedMedia {
    pub mime: String,
    pub base64: String,
    pub data_url: String,
    pub bytes: Vec<u8>,
}

/// `validateMedia`.
pub fn validate_media(
    route: &str,
    media_type: &str,
    data: &MediaData,
    supported: &[&str],
) -> Result<ValidatedMedia, LlmError> {
    let invalid = |m: String| LlmError::invalid_request(m);
    let mime = media_type.to_ascii_lowercase();
    if !supported.contains(&mime.as_str()) {
        return Err(invalid(format!(
            "{route} does not support media type {media_type}"
        )));
    }
    let base64 = match data {
        MediaData::Bytes(bytes) => {
            if bytes.len() > MAX_MEDIA_DECODED_BYTES {
                return Err(invalid(format!(
                    "{route} media exceeds the {MAX_MEDIA_DECODED_BYTES} byte decoded limit"
                )));
            }
            STANDARD.encode(bytes)
        }
        MediaData::Text(text) if text.starts_with("data:") => {
            let rest = &text[5..];
            let (header, payload) = rest.split_once(',').ok_or_else(|| {
                invalid(format!("{route} media data URL must contain valid base64"))
            })?;
            let declared = header
                .strip_suffix(";base64")
                .filter(|h| !h.is_empty() && !h.contains(';'));
            let Some(declared) = declared else {
                return Err(invalid(format!(
                    "{route} media data URL must contain valid base64"
                )));
            };
            if !payload
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
            {
                return Err(invalid(format!(
                    "{route} media data URL must contain valid base64"
                )));
            }
            if declared.to_ascii_lowercase() != mime {
                return Err(invalid(format!(
                    "{route} media type {media_type} does not match data URL type {declared}"
                )));
            }
            payload.to_owned()
        }
        MediaData::Text(text) => text.clone(),
    };
    if base64.len() > MAX_MEDIA_ENCODED_BYTES {
        return Err(invalid(format!(
            "{route} media exceeds the {MAX_MEDIA_ENCODED_BYTES} byte encoded limit"
        )));
    }
    let bytes = match STANDARD.decode(&base64) {
        Ok(bytes) if !base64.is_empty() && base64.len() % 4 == 0 => bytes,
        _ => return Err(invalid(format!("{route} media must contain valid base64"))),
    };
    if bytes.len() > MAX_MEDIA_DECODED_BYTES {
        return Err(invalid(format!(
            "{route} media exceeds the {MAX_MEDIA_DECODED_BYTES} byte decoded limit"
        )));
    }
    if STANDARD.encode(&bytes) != base64 {
        return Err(invalid(format!(
            "{route} media must contain canonical base64"
        )));
    }
    let data_url = format!("data:{mime};base64,{base64}");
    Ok(ValidatedMedia {
        mime,
        base64,
        data_url,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_validation_matches_baseline_messages() {
        let ok = validate_media(
            "r",
            "image/PNG",
            &MediaData::Text("data:image/png;base64,aGk=".into()),
            IMAGE_MIMES,
        )
        .unwrap();
        assert_eq!(ok.data_url, "data:image/png;base64,aGk=");
        let err = validate_media(
            "r",
            "image/png",
            &MediaData::Text("data:image/gif;base64,aGk=".into()),
            IMAGE_MIMES,
        )
        .unwrap_err();
        assert_eq!(
            err.reason.message(),
            "r media type image/png does not match data URL type image/gif"
        );
        let err = validate_media(
            "r",
            "image/png",
            &MediaData::Text("aGl".into()),
            IMAGE_MIMES,
        )
        .unwrap_err();
        assert_eq!(err.reason.message(), "r media must contain valid base64");
    }
}
