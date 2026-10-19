//! Purpose: wrapper for secret column values (tokens, share secrets, credential JSON) whose `Debug`
//! output is redacted. Column codecs live in codec.rs (`secret_text`, `secret_json`).
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! `-- storage-api/src/records/mod.rs
//! Invariants: `Debug` prints `Secret([REDACTED])`, never the value. Serialization is transparent so the
//! stored bytes equal the TS encoding.
//! Contract tests: storage-api/src/records/secret.rs, storage-sqlite/tests/credentials.rs

use serde::{Deserialize, Serialize};

/// A secret value. Access it explicitly with [`Secret::expose`].
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &T {
        &self.0
    }
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted_and_serde_transparent() {
        let secret = Secret::new("sk-live".to_owned());
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"sk-live\"");
    }
}
