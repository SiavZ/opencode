//! Purpose: console guarded support/admin actions (timing-safe bearer check, reset quota,
//! delete account, block/unblock workspaces, create referral) and the support lookup
//! (`packages/console/support/src/lib/lookup.ts`), plus operator script equivalents.
//! Scope: stub created by the phase-12 parent, filled by child node p12.ops.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/mod.rs
//! Invariants: owned by p12.ops. Child modules may live in a sibling directory of the same name.
//! Contract tests: none yet.
