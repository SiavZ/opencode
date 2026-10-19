//! Purpose: desktop updater ownership: the renderer-visible state machine, coalesced checks,
//! persisted ready target and channel, renderer subscription bookkeeping, and the Tauri
//! `latest.json` feed backend with minisign signature verification.
//! Scope: native desktop host. Does not depend on tauri, the shell adapts these types.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/updater/controller.rs
//! |-- desktop-host/src/updater/feed.rs
//! |-- desktop-host/src/updater/signature.rs
//! |-- desktop-host/src/updater/state.rs
//! |-- desktop-host/src/updater/store.rs
//! `-- desktop-host/src/updater/subscriptions.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lib.rs
//! |-- desktop-host/src/updater/feed.rs
//! |-- desktop-host/tests/updater_controller.rs
//! |-- desktop-host/tests/updater_feed.rs
//! |-- desktop-host/tests/updater_store.rs
//! `-- desktop-host/tests/updater_subscriptions.rs
//! Invariants: one authoritative state per controller, at most one check in flight, failed
//! checks and failed installs recover (error / ready) instead of wedging the state machine.
//! Contract tests: desktop-host/tests/updater_controller.rs, desktop-host/tests/updater_feed.rs,
//! desktop-host/tests/updater_store.rs

mod controller;
mod feed;
mod signature;
mod state;
mod store;
mod subscriptions;

pub use controller::{
    CheckResult, StopFn, UpdaterBackend, UpdaterController, UpdaterInput, UpdaterPersistence,
};
pub use feed::{
    FeedBackend, FeedConfig, FeedPlatform, FeedRelease, Installer, UnsupportedInstaller,
};
pub use signature::{MinisignVerifier, SignatureVerifier};
pub use state::{UpdaterError, UpdaterReadyRecord, UpdaterState};
pub use store::{DEFAULT_CHANNEL, UPDATER_STORE, UpdaterFileStore};
pub use subscriptions::UpdaterSubscriptions;
