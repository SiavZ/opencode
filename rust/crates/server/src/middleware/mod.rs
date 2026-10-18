//! Purpose: HTTP middleware: auth, CORS, request decoding, error envelopes, legacy
//! directory/workspace routing.
//! Scope: per-request layers.
//! Uses:
//! rust/crates/
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/compression.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/middleware/decode.rs
//! |-- server/src/middleware/errors.rs
//! |-- server/src/middleware/legacy_routing.rs
//! `-- server/src/middleware/location.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- server/src/lib.rs
//! Invariants: owner p06.server-http. Middleware owns no domain policy.
//! Contract tests: server/tests/http_auth.rs, server/tests/http_cors.rs, server/tests/http_proxy.rs

pub mod auth;
pub mod compression;
pub mod cors;
pub mod decode;
pub mod errors;
pub mod legacy_routing;
pub mod location;
