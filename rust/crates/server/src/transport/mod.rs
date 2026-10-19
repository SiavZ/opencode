//! Purpose: transport adapters: SSE, WebSocket, remote proxy.
//! Scope: per-connection transports.
//! Uses:
//! rust/crates/
//! |-- server/src/transport/proxy.rs
//! |-- server/src/transport/sse.rs
//! `-- server/src/transport/websocket.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- server/src/lib.rs
//! Invariants: slot reserved, owner p06.server-http.
//! Contract tests: none yet

pub mod proxy;
pub mod proxy_socket;
pub mod sse;
pub mod websocket;
