//! Purpose: transport adapters: SSE, WebSocket, remote proxy.
//! Scope: per-connection transports.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- server/src/lib.rs
//! Invariants: slot reserved, owner p06.server-http.
//! Contract tests: none yet

pub mod proxy;
pub mod sse;
pub mod websocket;
