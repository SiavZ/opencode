//! Purpose: crate root of the portable session view-model shared by the browser (Phase 10)
//! and terminal (Phase 09) renderers.
//! Scope: pure data transforms, no I/O, no renderer, compiles for wasm32-unknown-unknown.
//! Uses:
//! rust/crates/
//! |-- view-model/src/actions.rs
//! |-- view-model/src/identity.rs
//! |-- view-model/src/projection/mod.rs
//! |-- view-model/src/reducer/mod.rs
//! |-- view-model/src/session_cache.rs
//! |-- view-model/src/source.rs
//! `-- view-model/src/timeline/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- view-model/tests/projection.rs
//! |-- view-model/tests/reducer.rs
//! |-- view-model/tests/session_cache.rs
//! `-- view-model/tests/timeline.rs
//! Invariants: never depends on server, runtime, session-runtime, storage or platform crates.
//! Contract tests: view-model/tests/{reducer,projection,timeline,identity,session_cache}.rs
//!
//! # Public API (stable for Phase 09 and Phase 10 consumers)
//!
//! Data flow: `SessionEvent` -> [`session_cache::SessionCache::apply_event`] (wraps
//! [`V2SessionReducer`]) -> canonical `Vec<SessionMessage>` -> [`projection::project`] ->
//! [`projection::Projection`] (render messages + parts with baseline-stable ids) ->
//! [`timeline::construct_rows`] -> keyed [`timeline::TimelineRow`]s -> [`timeline::reuse_rows`]
//! to keep context-group identity stable between recomputations.
//!
//! - [`source`]: wire-shaped canonical values (`SessionMessage`, `SessionEvent`, tool states).
//! - [`reducer`]: `V2SessionReducer::reduce(&[SessionMessage], &SessionEvent) -> Option<Reduction>`.
//! - [`session_cache`]: per-server cache with page/live merge (`begin_load`, `apply_page`,
//!   `remove_message`, `hydrate`), status, optimistic roots, pins and LRU eviction.
//! - [`projection`]: `project`, `part_id`, `message_key`, `RenderMessage`, `RenderPart`, `PartKind`.
//! - [`timeline`]: `construct_rows`, `renderable`, `group_parts`, `reuse_rows`, row keys,
//!   `visible_user_messages`, `is_timeline_ready`, text helpers (`reasoning_heading`, ...).
//! - [`identity`]: `ServerKey`, `SessionKey`, `DraftKey`, `Target`, route encoding, `Ownership`.
//! - [`actions`]: `PromptSubmit` state machine emitting target-scoped `Effect`s.
//!
//! Remount ownership: the browser keys transport and cache by `ServerKey` (session changes do not
//! recreate the server root); the terminal may key its view by `SessionKey`. Both use the same rows.

pub mod actions;
pub mod browser;
pub mod identity;
pub mod projection;
pub mod reducer;
pub mod session_cache;
pub mod source;
pub mod timeline;

pub use reducer::{Reduction, V2SessionReducer};
pub use source::{SessionEvent, SessionMessage};
