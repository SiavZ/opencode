//! Purpose: AWS Signature V4 header signing that reproduces `aws4fetch` 1.0.20 as used by
//! packages/enterprise/src/core/storage.ts (service s3, `UNSIGNED-PAYLOAD`, R2 region `auto`).
//! Scope: pure signing over method/url/headers/time, no I/O.
//! Uses:
//! `-- none
//! External:
//! |-- hmac::{Hmac, KeyInit, Mac}
//! |-- percent_encoding
//! |-- sha2::{Digest, Sha256}
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/mod.rs
//! `-- hosted-adapters/src/sharing/objects.rs
//! Invariants: canonical path is decode-then-encodeURIComponent with `/` kept and `!'()*`
//! escaped, query pairs are de-duplicated (s3) then sorted, signed headers are `host` plus
//! every set header except unsignable ones, lowercased and sorted.
//! Contract tests: hosted-adapters/tests/sharing_baseline.rs (vectors captured from aws4fetch)

use hmac::{Hmac, KeyInit, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};
use url::Url;

/// `encodeURIComponent` keeps `A-Za-z0-9-_.!~*'()`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

#[derive(Debug, Clone)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

fn rfc3986(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '!' | '\'' | '(' | ')' | '*' => out.push_str(&format!("%{:X}", c as u32)),
            other => out.push(other),
        }
    }
    out
}

fn encode_component(input: &str) -> String {
    rfc3986(&utf8_percent_encode(input, URI_COMPONENT).to_string())
}

/// `guessServiceRegion` for the two hosts the baseline uses.
pub fn region_for(url: &Url, fallback: &str) -> String {
    let host = url.host_str().unwrap_or_default();
    if host.ends_with(".r2.cloudflarestorage.com") {
        return "auto".into();
    }
    if let Some(rest) = host.strip_prefix("s3.")
        && let Some(region) = rest.strip_suffix(".amazonaws.com")
    {
        return region.into();
    }
    fallback.into()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Returns the full header list to send (in signing order, including `authorization`,
/// `x-amz-content-sha256` and `x-amz-date`). `datetime` is `YYYYMMDDTHHMMSSZ`.
pub fn sign(
    creds: &Credentials,
    method: &str,
    url: &Url,
    headers: &[(String, String)],
    region: &str,
    datetime: &str,
) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .filter(|(k, _)| k != "host")
        .collect();
    if !all.iter().any(|(k, _)| k == "x-amz-content-sha256") {
        all.push(("x-amz-content-sha256".into(), "UNSIGNED-PAYLOAD".into()));
    }
    all.push(("x-amz-date".into(), datetime.into()));
    let unsignable = [
        "authorization",
        "content-type",
        "content-length",
        "user-agent",
        "presigned-expires",
        "expect",
        "x-amzn-trace-id",
        "range",
        "connection",
    ];
    let mut signable: Vec<String> = std::iter::once("host".to_string())
        .chain(all.iter().map(|(k, _)| k.clone()))
        .filter(|k| !unsignable.contains(&k.as_str()))
        .collect();
    signable.sort();
    signable.dedup();
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let canonical_headers: String = signable
        .iter()
        .map(|h| {
            let value = if h == "host" {
                host.clone()
            } else {
                let raw = all
                    .iter()
                    .find(|(k, _)| k == h)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or_default();
                raw.split_whitespace().collect::<Vec<_>>().join(" ")
            };
            format!("{h}:{value}\n")
        })
        .collect();
    let signed_headers = signable.join(";");
    let decoded = percent_decode_str(&url.path().replace('+', " "))
        .decode_utf8_lossy()
        .to_string();
    let path = encode_component(&decoded).replace("%2F", "/");
    let mut seen = Vec::new();
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            if k.is_empty() || seen.contains(&k.to_string()) {
                return false;
            }
            seen.push(k.to_string());
            true
        })
        .map(|(k, v)| (encode_component(&k), encode_component(&v)))
        .collect();
    pairs.sort();
    let query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let payload = all
        .iter()
        .find(|(k, _)| k == "x-amz-content-sha256")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let canonical = [
        method,
        &path,
        &query,
        &canonical_headers,
        &signed_headers,
        &payload,
    ]
    .join("\n");
    let date = &datetime[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = [
        "AWS4-HMAC-SHA256",
        datetime,
        &scope,
        &hex::encode(Sha256::digest(canonical.as_bytes())),
    ]
    .join("\n");
    let k_date = hmac(format!("AWS4{}", creds.secret_access_key).as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));
    all.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            creds.access_key_id
        ),
    ));
    all
}
