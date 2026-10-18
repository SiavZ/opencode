//! Purpose: typed error for provider/protocol/auth behavior that is not ported yet, so
//! callers get an explicit, tested failure instead of a silent fallback.
//! Scope: pure value.
//! Uses:
//! `-- none
//! External:
//! `-- opencode_llm_api::{LlmError, LlmErrorReason}
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: maps to `LlmErrorReason::NoRoute` (non-retryable) carrying the unported route
//! id, so session code treats it like an unsupported API rather than a transient failure.
//! Contract tests: provider-adapters/src/unsupported.rs (unit tests)

use opencode_llm_api::{LlmError, LlmErrorReason, ModelId, ProviderId};

/// `NoRoute` for `provider/model` over a route that is recorded as not yet ported.
pub fn not_yet_ported(provider: &ProviderId, model: &ModelId, route: &str) -> LlmError {
    LlmError::new(
        "ProviderAdapters",
        "resolve",
        LlmErrorReason::NoRoute {
            route: format!("{route} (not yet ported)"),
            provider: provider.clone(),
            model: model.clone(),
        },
    )
}
