//! Purpose: console adapters (MySQL, auth issuer, Stripe, rate limits, gateway, mail, support).
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/billing_store.rs
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/gateway.rs
//! |-- hosted-adapters/src/console/limits.rs
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/stripe.rs
//! |-- hosted-adapters/src/console/support.rs
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/lib.rs
//! Invariants: owned by phase-12; children own one file each (see contracts/phase-12/contract.md).
//! Contract tests: hosted-adapters/tests/console_*.rs

pub mod auth;
pub mod billing_store;
pub mod db;
pub mod formats;
pub mod gateway;
pub mod limits;
pub mod mail;
pub mod public;
pub mod referral;
pub mod stripe;
pub mod support;
pub mod tenant;
pub mod workers;
