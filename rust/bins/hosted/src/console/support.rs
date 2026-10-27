//! Purpose: guarded console support actions, operator lookup and enterprise inquiry delivery.
//! Scope: support HTTP handlers with startup-injected secrets and integration configuration.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/state.rs
//! |-- hosted/src/console/support/actions.rs
//! |-- hosted/src/console/support/enterprise.rs
//! |-- hosted/src/console/support/honeycomb.rs
//! |-- hosted/src/console/support/locale.rs
//! |-- hosted/src/console/support/lookup.rs
//! |-- hosted/src/console/support/tests.rs
//! `-- hosted/src/console/support/tests_actions.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! External:
//! `-- axum::Router
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/mod.rs
//! `-- hosted/src/console/state.rs
//! Invariants: support authentication precedes body parsing; secrets never appear in Debug.
//! Contract tests: hosted/src/console/support/tests.rs

mod actions;
#[cfg(test)]
#[path = "support/browser_tests.rs"]
mod browser_tests;
mod enterprise;
mod honeycomb;
mod locale;
mod lookup;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_actions;

use axum::Router;
use axum::routing::{delete, get, post};

use super::ConsoleState;

#[derive(Clone, Default)]
pub struct Config {
    pub api_key: Option<String>,
    pub enterprise: opencode_hosted_adapters::console::support::enterprise::EnterpriseConfig,
    pub honeycomb_secret: Option<String>,
    pub discord_webhook_url: Option<String>,
    pub limits: Option<serde_json::Value>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("api_key_configured", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            api_key: std::env::var("SUPPORT_API_KEY")
                .ok()
                .filter(|v| !v.is_empty()),
            enterprise:
                opencode_hosted_adapters::console::support::enterprise::EnterpriseConfig::from_env(),
            honeycomb_secret: std::env::var("HoneycombWebhookSecret")
                .or_else(|_| std::env::var("HONEYCOMB_WEBHOOK_SECRET"))
                .ok()
                .filter(|v| !v.is_empty()),
            discord_webhook_url: std::env::var("DISCORD_INCIDENT_WEBHOOK_URL")
                .ok()
                .filter(|v| !v.is_empty()),
            limits: std::env::var("ZEN_LIMITS")
                .ok()
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| anyhow::anyhow!("ZEN_LIMITS must be valid JSON"))?,
        })
    }
}

pub fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/api/enterprise", post(enterprise::submit))
        .route("/api/email-signup", post(enterprise::email_signup))
        .route("/honeycomb/webhook", post(honeycomb::webhook))
        .route("/api/support/lookup", get(lookup::lookup))
        .route(
            "/api/support/actions/reset-quota",
            post(actions::reset_quota),
        )
        .route(
            "/api/support/actions/create-referral",
            post(actions::create_referral),
        )
        .route(
            "/api/support/actions/block-workspaces",
            post(actions::block),
        )
        .route(
            "/api/support/actions/unblock-workspaces",
            post(actions::unblock_batch),
        )
        .route(
            "/api/support/actions/unblock-workspace",
            post(actions::unblock),
        )
        .route(
            "/api/support/actions/delete-account",
            delete(actions::delete_account),
        )
}
