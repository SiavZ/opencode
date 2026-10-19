//! Purpose: update artifact signature verification hook. The production verifier checks
//! minisign signatures in the Tauri updater encoding (base64 of the minisign `.pub` file and
//! base64 of the `.sig` file).
//! Scope: pure verification, no IO, no key generation (test keys are generated in tests only).
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater/state.rs
//! External:
//! |-- base64::Engine
//! |-- base64::engine::general_purpose::STANDARD
//! |-- minisign_verify::PublicKey
//! `-- minisign_verify::Signature
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! Invariants: verification fails closed, legacy (non-prehashed) signatures are rejected like
//! the Tauri updater does, a key id mismatch is a signature failure.
//! Contract tests: desktop-host/tests/updater_feed.rs

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{PublicKey, Signature};

use super::state::UpdaterError;

/// Hook used by the feed backend before any downloaded bytes are staged.
pub trait SignatureVerifier: Send + Sync + 'static {
    fn verify(&self, bytes: &[u8], signature: &str) -> Result<(), UpdaterError>;
}

#[derive(Debug)]
pub struct MinisignVerifier {
    key: PublicKey,
}

impl MinisignVerifier {
    /// `pubkey` is the `plugins.updater.pubkey` value of a Tauri config: base64 of the full
    /// minisign public key file (comment line plus key line).
    pub fn from_tauri_pubkey(pubkey: &str) -> Result<Self, UpdaterError> {
        let decoded = decode_text(pubkey)?;
        let key = PublicKey::decode(&decoded)
            .map_err(|error| UpdaterError::Signature(format!("invalid public key: {error}")))?;
        Ok(Self { key })
    }

    /// Raw minisign public key line (base64 key bytes without the comment).
    pub fn from_minisign_base64(key: &str) -> Result<Self, UpdaterError> {
        let key = PublicKey::from_base64(key.trim())
            .map_err(|error| UpdaterError::Signature(format!("invalid public key: {error}")))?;
        Ok(Self { key })
    }
}

impl SignatureVerifier for MinisignVerifier {
    fn verify(&self, bytes: &[u8], signature: &str) -> Result<(), UpdaterError> {
        let decoded = decode_text(signature)?;
        let signature = Signature::decode(&decoded)
            .map_err(|error| UpdaterError::Signature(format!("invalid signature: {error}")))?;
        self.key
            .verify(bytes, &signature, false)
            .map_err(|error| UpdaterError::Signature(error.to_string()))
    }
}

fn decode_text(value: &str) -> Result<String, UpdaterError> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|error| UpdaterError::Signature(format!("invalid base64: {error}")))?;
    String::from_utf8(bytes)
        .map_err(|error| UpdaterError::Signature(format!("invalid utf-8: {error}")))
}
