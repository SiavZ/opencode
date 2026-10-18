//! Purpose: console domain rules (identity, membership, credentials, billing, referrals, quotas,
//! inference). Pure decisions over values, no I/O.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- hosted-domain/src/console/billing.rs
//! |-- hosted-domain/src/console/common.rs
//! |-- hosted-domain/src/console/credentials.rs
//! |-- hosted-domain/src/console/identity.rs
//! |-- hosted-domain/src/console/inference.rs
//! |-- hosted-domain/src/console/membership.rs
//! |-- hosted-domain/src/console/quotas.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- hosted-domain/src/lib.rs
//! Invariants: owned by phase-12; children own one file each (see contracts/phase-12/contract.md).
//! Contract tests: unit tests in each module.

pub mod billing;
pub mod common;
pub mod credentials;
pub mod identity;
pub mod inference;
pub mod membership;
pub mod quotas;
pub mod referral;
