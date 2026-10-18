//! Purpose: Fe26.2 "iron" seal/unseal compatible with `iron-webcrypto` as used by h3
//! `useSession` (the console `auth` cookie), plus the h3 session envelope
//! `{id, createdAt, data}` with maxAge enforcement.
//! Scope: pure crypto over strings; the caller supplies time and randomness.
//! Uses:
//! `-- none
//! External:
//! |-- aes::Aes256
//! |-- cbc::{Decryptor, Encryptor}
//! |-- hmac::Hmac
//! |-- pbkdf2::pbkdf2_hmac
//! `-- sha1::Sha1
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth.rs
//! Invariants: format `Fe26.2*<id>*<encSalt>*<iv b64url>*<ct b64url>*<exp>*<macSalt>*<mac b64url>`;
//! keys = PBKDF2-SHA1(password, hex salt string, 1 iteration, 32 bytes); AES-256-CBC with PKCS7;
//! HMAC-SHA256 over the base string; 256-bit hex salts; password >= 32 chars; expiry checked
//! with 60s skew (`iron-webcrypto@1.2.1 dist/index.js:90-280`, `h3@1.15 sealSession`).
//! Contract tests: hosted-adapters/src/console/auth/iron.rs (unit tests)

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const MAC_PREFIX: &str = "Fe26.2";
const MIN_PASSWORD: usize = 32;
const SKEW_MS: i64 = 60_000;

/// Seal/unseal failure. Unseal failures are treated as "no session" by h3.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IronError {
    #[error("Password string too short (min 32 characters required)")]
    PasswordTooShort,
    #[error("Incorrect number of sealed components")]
    Components,
    #[error("Wrong mac prefix")]
    Prefix,
    #[error("Invalid expiration")]
    InvalidExpiration,
    #[error("Expired seal")]
    Expired,
    #[error("Bad hmac value")]
    BadMac,
    #[error("Decryption failed")]
    Decrypt,
    #[error("Session expired!")]
    SessionExpired,
    #[error("Invalid session payload")]
    Payload,
}

fn derive(password: &str, salt: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), salt.as_bytes(), 1, &mut key);
    key
}

fn mac(password: &str, salt: &str, base: &str) -> String {
    let key = derive(password, salt);
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("any key length");
    m.update(base.as_bytes());
    URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
}

/// Randomness for one seal: two 32-byte salts and a 16-byte IV.
#[derive(Debug, Clone, Copy)]
pub struct SealRandom {
    pub enc_salt: [u8; 32],
    pub mac_salt: [u8; 32],
    pub iv: [u8; 16],
}

impl SealRandom {
    pub fn os() -> Self {
        let mut buf = [0u8; 80];
        getrandom::fill(&mut buf).expect("OS random source unavailable");
        let mut r = SealRandom {
            enc_salt: [0; 32],
            mac_salt: [0; 32],
            iv: [0; 16],
        };
        r.enc_salt.copy_from_slice(&buf[..32]);
        r.mac_salt.copy_from_slice(&buf[32..64]);
        r.iv.copy_from_slice(&buf[64..]);
        r
    }
}

/// `seal(object, password, { ttl })` with `ttl_ms = 0` meaning no expiration.
pub fn seal(
    json: &str,
    password: &str,
    ttl_ms: i64,
    now_ms: i64,
    random: SealRandom,
) -> Result<String, IronError> {
    if password.len() < MIN_PASSWORD {
        return Err(IronError::PasswordTooShort);
    }
    let enc_salt = hex::encode(random.enc_salt);
    let key = derive(password, &enc_salt);
    let ct = cbc::Encryptor::<aes::Aes256>::new(&key.into(), &random.iv.into())
        .encrypt_padded_vec::<Pkcs7>(json.as_bytes());
    let expiration = if ttl_ms > 0 {
        (now_ms + ttl_ms).to_string()
    } else {
        String::new()
    };
    let base = format!(
        "{MAC_PREFIX}**{enc_salt}*{}*{}*{expiration}",
        URL_SAFE_NO_PAD.encode(random.iv),
        URL_SAFE_NO_PAD.encode(&ct)
    );
    let mac_salt = hex::encode(random.mac_salt);
    let digest = mac(password, &mac_salt, &base);
    Ok(format!("{base}*{mac_salt}*{digest}"))
}

/// `unseal(sealed, password, opts)`: returns the decrypted JSON text.
pub fn unseal(sealed: &str, password: &str, now_ms: i64) -> Result<String, IronError> {
    let parts: Vec<&str> = sealed.split('*').collect();
    if parts.len() != 8 {
        return Err(IronError::Components);
    }
    if parts[0] != MAC_PREFIX {
        return Err(IronError::Prefix);
    }
    if !parts[5].is_empty() {
        if !parts[5].bytes().all(|b| b.is_ascii_digit()) {
            return Err(IronError::InvalidExpiration);
        }
        let exp: i64 = parts[5].parse().map_err(|_| IronError::InvalidExpiration)?;
        if exp <= now_ms - SKEW_MS {
            return Err(IronError::Expired);
        }
    }
    if password.len() < MIN_PASSWORD {
        return Err(IronError::PasswordTooShort);
    }
    let base = parts[..6].join("*");
    let expected = mac(password, parts[6], &base);
    if !constant_time_eq(expected.as_bytes(), parts[7].as_bytes()) {
        return Err(IronError::BadMac);
    }
    let ct = URL_SAFE_NO_PAD
        .decode(parts[4])
        .map_err(|_| IronError::Decrypt)?;
    let iv: [u8; 16] = URL_SAFE_NO_PAD
        .decode(parts[3])
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(IronError::Decrypt)?;
    let key = derive(password, parts[2]);
    let pt = cbc::Decryptor::<aes::Aes256>::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(&ct)
        .map_err(|_| IronError::Decrypt)?;
    String::from_utf8(pt).map_err(|_| IronError::Decrypt)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    let b = if a.len() == b.len() { b } else { a };
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// h3 session envelope: `{ id, createdAt, data }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct H3Session<T> {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: i64,
    pub data: T,
}

/// `unsealSession`: unseal plus `maxAge` check on `createdAt`.
pub fn unseal_session<T: for<'de> Deserialize<'de>>(
    sealed: &str,
    password: &str,
    max_age_s: i64,
    now_ms: i64,
) -> Result<H3Session<T>, IronError> {
    let json = unseal(sealed, password, now_ms)?;
    let session: H3Session<T> = serde_json::from_str(&json).map_err(|_| IronError::Payload)?;
    if max_age_s > 0 && now_ms - session.created_at > max_age_s * 1000 {
        return Err(IronError::SessionExpired);
    }
    Ok(session)
}

/// `sealSession`: ttl = maxAge seconds.
pub fn seal_session<T: Serialize>(
    session: &H3Session<T>,
    password: &str,
    max_age_s: i64,
    now_ms: i64,
    random: SealRandom,
) -> Result<String, IronError> {
    let json = serde_json::to_string(session).map_err(|_| IronError::Payload)?;
    seal(&json, password, max_age_s * 1000, now_ms, random)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "fixture-session-secret-0123456789abcdef";

    fn fixed() -> SealRandom {
        SealRandom {
            enc_salt: [1; 32],
            mac_salt: [2; 32],
            iv: [3; 16],
        }
    }

    #[test]
    fn roundtrip_and_tamper() {
        let sealed = seal(r#"{"a":1}"#, PASSWORD, 1000, 10_000, fixed()).unwrap();
        assert!(sealed.starts_with("Fe26.2**0101"));
        assert_eq!(sealed.split('*').count(), 8);
        assert_eq!(unseal(&sealed, PASSWORD, 10_500).unwrap(), r#"{"a":1}"#);
        assert_eq!(unseal(&sealed, PASSWORD, 80_000), Err(IronError::Expired));
        let wrong = "fixture-session-secret-XXXXXXXXXXXXXXXX";
        assert_eq!(unseal(&sealed, wrong, 10_500), Err(IronError::BadMac));
        let tampered = sealed.replacen("Fe26.2", "Fe26.1", 1);
        assert_eq!(unseal(&tampered, PASSWORD, 10_500), Err(IronError::Prefix));
        assert_eq!(
            seal("{}", "short", 0, 0, fixed()),
            Err(IronError::PasswordTooShort)
        );
    }

    /// Golden value from `iron-webcrypto@1.2.1` via
    /// `contracts/phase-12/fixtures/identity/iron-capture.mjs` (fixture).
    #[test]
    fn fixture_matches_iron_webcrypto_bytes() {
        let golden = "Fe26.2**0101010101010101010101010101010101010101010101010101010101010101*AwMDAwMDAwMDAwMDAwMDAw*W2eem52ykth-sceKq1kLyA*11000*0202020202020202020202020202020202020202020202020202020202020202*Sfpq031zr1N-cB7vf1lZRMMiXHyN8c6dMPXQPUoOfSA";
        assert_eq!(
            seal(r#"{"a":1}"#, PASSWORD, 1000, 10_000, fixed()).unwrap(),
            golden
        );
        assert_eq!(unseal(golden, PASSWORD, 10_000).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn session_max_age() {
        let s = H3Session {
            id: "x".into(),
            created_at: 1_000,
            data: serde_json::json!({}),
        };
        let sealed = seal_session(&s, PASSWORD, 10, 1_000, fixed()).unwrap();
        let back: H3Session<serde_json::Value> =
            unseal_session(&sealed, PASSWORD, 10, 5_000).unwrap();
        assert_eq!(back, s);
        assert!(unseal_session::<serde_json::Value>(&sealed, PASSWORD, 10, 12_000).is_err());
    }
}
