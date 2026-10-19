//! Purpose: ecosystem integrations ported from the TypeScript baseline: GitHub Action/CLI
//! automation and token exchange, Feishu-to-Discord bridge, Slack Socket Mode bot, VS Code
//! editor command/context logic and first-party triage tools. Does not own session
//! execution, share hosting or the CLI argument surface (phase 09 calls into this crate).
//! Scope: library, per-run and per-connection owned state.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/bridge.rs
//! |-- ecosystem/src/editor/mod.rs
//! |-- ecosystem/src/github/mod.rs
//! |-- ecosystem/src/session.rs
//! `-- ecosystem/src/slack/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: no module performs live network calls in tests. Every external host is an
//! injected base URL so fixture servers stand in for GitHub, Slack, Discord and opencode.
//! Contract tests: ecosystem/tests/
pub mod bridge;
pub mod editor;
pub mod github;
pub mod session;
pub mod slack;
