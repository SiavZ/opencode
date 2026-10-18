//! Purpose: server health probing (`checkHealth`): GET `/api/health` then `/global/health` with a
//! 3s per-request timeout and optional basic auth; any 2xx is healthy.
//! Scope: single probe over a caller-supplied reqwest client (proxy/CA policy lives in platform/proxy).
//! Uses:
//! `-- none
//! External:
//! |-- reqwest::Client
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/connections.rs
//! |-- desktop-host/src/connections/child.rs
//! `-- desktop-host/tests/connections_live.rs
//! Invariants: an unparsable base URL is unhealthy (no request), network errors are unhealthy, never panics.
//! Contract tests: desktop-host/tests/connections_live.rs

use std::time::Duration;

/// Per-request timeout (`AbortSignal.timeout(3000)`, `packages/desktop/src/main/server.ts:202`).
pub const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Paths probed in order (`server.ts:187`).
pub const HEALTH_PATHS: [&str; 2] = ["/api/health", "/global/health"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl BasicAuth {
    /// Baseline health checks always authenticate as `opencode` (`server.ts:194`).
    pub fn opencode(password: &str) -> Option<Self> {
        if password.is_empty() {
            return None;
        }
        Some(Self {
            username: super::SERVER_USERNAME.to_string(),
            password: password.to_string(),
        })
    }
}

pub async fn check_health(client: &reqwest::Client, url: &str, auth: Option<&BasicAuth>) -> bool {
    let Ok(base) = url::Url::parse(url) else {
        return false;
    };
    for path in HEALTH_PATHS {
        let Ok(target) = base.join(path) else {
            return false;
        };
        let mut request = client.get(target).timeout(HEALTH_REQUEST_TIMEOUT);
        if let Some(auth) = auth {
            request = request.basic_auth(&auth.username, Some(&auth.password));
        }
        if let Ok(response) = request.send().await
            && response.status().is_success()
        {
            return true;
        }
    }
    false
}
