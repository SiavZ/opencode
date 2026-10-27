//! Purpose: authenticate support operators and expose durable support lookup and operations.
//! Scope: per-request support authorization and MySQL repositories.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/support/enterprise.rs
//! |-- hosted-adapters/src/console/support/lookup.rs
//! |-- hosted-adapters/src/console/support/presentation.rs
//! |-- hosted-adapters/src/console/support/quota.rs
//! |-- hosted-adapters/src/console/support/referral.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- hmac::{Hmac, KeyInit, Mac}
//! `-- sha2::Sha256
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! rust/crates/
//! |-- hosted-adapters/tests/console_support.rs
//! `-- hosted-adapters/tests/console_support_enterprise.rs
//! Invariants: empty configuration fails closed; bearer comparison uses constant-time MAC verification.
//! Contract tests: hosted-adapters/tests/console_support.rs

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

pub mod enterprise;
mod lookup;
mod presentation;
mod quota;
mod referral;

use super::db::ConsoleDb;
use super::tenant::TenantError;

pub type SupportResult<T> = Result<T, TenantError>;

#[derive(Clone)]
pub struct SupportStore {
    db: ConsoleDb,
}

impl std::fmt::Debug for SupportStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupportStore").finish_non_exhaustive()
    }
}

impl SupportStore {
    pub fn new(db: ConsoleDb) -> Self {
        Self { db }
    }
}

/// Compare the complete, case-sensitive Authorization value without secret-dependent comparisons.
/// MAC verification is constant-time even when the untrusted header has a different length.
pub fn authorized(header: Option<&str>, secret: Option<&str>) -> bool {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return false;
    };
    let Some(header) = header else {
        return false;
    };
    secure_equal(header, &format!("Bearer {secret}"))
}

pub fn secure_equal(actual: &str, expected: &str) -> bool {
    let mut expected_mac =
        Hmac::<Sha256>::new_from_slice(expected.as_bytes()).expect("HMAC accepts keys of any size");
    expected_mac.update(expected.as_bytes());
    let mut actual_mac =
        Hmac::<Sha256>::new_from_slice(expected.as_bytes()).expect("HMAC accepts keys of any size");
    actual_mac.update(actual.as_bytes());
    actual_mac
        .verify_slice(&expected_mac.finalize().into_bytes())
        .is_ok()
}
