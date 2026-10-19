//! Purpose: virtualized session timeline: windowing, measurement cache, bottom anchoring, row rendering.
//! Scope: timeline rendering.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.session; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.session)

pub const DEFAULT_ROW_ESTIMATE_PX: f64 = 96.0;
