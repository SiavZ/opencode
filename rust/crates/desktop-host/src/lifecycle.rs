//! Purpose: desktop process lifecycle: startup readiness, single-instance arbitration, quit and
//! relaunch coordination, onboarding flags and env-derived startup configuration.
//! Scope: module root, re-exports child modules. Platform calls (window creation, app.relaunch)
//! stay in the shell, this module only decides and coordinates.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/lifecycle/config.rs
//! |-- desktop-host/src/lifecycle/onboarding.rs
//! |-- desktop-host/src/lifecycle/quit.rs
//! |-- desktop-host/src/lifecycle/readiness.rs
//! `-- desktop-host/src/lifecycle/single_instance.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! Invariants: see child modules.
//! Contract tests: desktop-host/tests/lifecycle_*.rs

pub mod config;
pub mod onboarding;
pub mod quit;
pub mod readiness;
pub mod single_instance;
