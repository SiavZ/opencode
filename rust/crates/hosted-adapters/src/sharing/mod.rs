//! Purpose: sharing adapters: SigV4 signing, the S3/R2 object store, the enterprise share service
//! and the in-process public share Durable Object host.
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! |-- hosted-adapters/src/sharing/public.rs
//! |-- hosted-adapters/src/sharing/service.rs
//! `-- hosted-adapters/src/sharing/sigv4.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/lib.rs
//! Invariants: owned by phase-13 only, see rust/contracts/phase-90/execution-map.md.
//! Contract tests: hosted-adapters/tests/sharing_baseline.rs, hosted-adapters/tests/sharing_public.rs

pub mod objects;
pub mod public;
pub mod service;
pub mod sigv4;
