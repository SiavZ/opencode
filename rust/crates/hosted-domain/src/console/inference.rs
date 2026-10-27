//! Purpose: resolve public Zen paths to the migrated inference service.
//! Scope: pure route and authentication-header selection, without provider calls.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway.rs
//! Invariants: only supported routes are forwarded; custom connection IDs match migration imports.
//! Contract tests: hosted-adapters/tests/console_gateway.rs

/// The destination owns authentication, rate limits and accounting after cutover.
pub fn migrated_path(method: &str, path: &str) -> Option<String> {
    let destination = match (method, path) {
        ("POST", "/zen/v1/chat/completions") => "/openai/v1/chat/completions",
        ("POST", "/zen/v1/responses") => "/openai/v1/responses",
        ("POST", "/zen/v1/messages") => "/anthropic/v1/messages",
        ("POST", "/zen/v1/systemone") => "/systemone/v1/systemone",
        ("POST", "/zen/go/v1/chat/completions") => "/go/openai/v1/chat/completions",
        ("POST", "/zen/go/v1/responses") => "/go/openai/v1/responses",
        ("POST", "/zen/go/v1/messages") => "/go/anthropic/v1/messages",
        ("POST", "/zen/go/v1/systemone") => "/go/systemone/v1/systemone",
        ("GET", "/zen/v1/models") => "/v1/models",
        ("GET", "/zen/go/v1/models") => "/go/v1/models",
        ("GET", "/zen/go/v1/usage") => "/go/v1/usage",
        ("POST", path) => {
            let model = path.strip_prefix("/zen/v1/models/")?;
            let (model, action) = model.rsplit_once(':')?;
            if model.is_empty()
                || model.contains('/')
                || !matches!(action, "generateContent" | "streamGenerateContent")
            {
                return None;
            }
            return Some(format!("/google/v1beta/models/{model}:{action}"));
        }
        _ => return None,
    };
    Some(destination.into())
}

pub fn api_key_header(path: &str) -> &'static str {
    if path.ends_with("/messages") {
        return "x-api-key";
    }
    if path.starts_with("/zen/v1/models/") {
        return "x-goog-api-key";
    }
    "authorization"
}

pub fn custom_connection_path(
    path: &str,
    workspace: &str,
    provider: &str,
    encoded_model: &str,
) -> Option<String> {
    let workspace = workspace.strip_prefix("wrk_")?;
    let suffix = if path.starts_with("/zen/v1/models/") {
        format!("/models/{encoded_model}:{}", path.rsplit_once(':')?.1)
    } else {
        path.strip_prefix("/zen/v1")?.to_owned()
    };
    Some(format!("/custom/conn_{workspace}_{provider}{suffix}"))
}
