//! Purpose: deliver finalized native gateway metrics as baseline trace-tail envelopes.
//! Scope: synchronous owned per-request HTTP delivery to explicitly configured native ingress.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::{Context,Result}
//! |-- reqwest::Client
//! |-- serde_json::{Value,json}
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/workers.rs
//! `-- hosted/src/console/zen/completion.rs
//! Invariants: no detached tasks; HTTP deadline is five seconds; auth configured before requests; redirects forbidden.
//! Contract tests: hosted/src/console/workers/tail_tests.rs

use anyhow::{Context, Result};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct TailEmitter {
    client: reqwest::Client,
    endpoint: url::Url,
    token: String,
}

impl std::fmt::Debug for TailEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TailEmitter")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

impl TailEmitter {
    pub fn new(endpoint: &str) -> Result<Self> {
        let token = std::env::var("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN")
            .context("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN is required for native tail emission")?;
        Self::with_token(endpoint, &token)
    }

    pub fn with_token(endpoint: &str, token: &str) -> Result<Self> {
        anyhow::ensure!(!token.is_empty(), "native tail token must not be empty");
        let endpoint = url::Url::parse(endpoint).context("invalid native tail endpoint")?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https")
                && endpoint.host_str().is_some_and(|host| host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())),
            "native tail endpoint must be literal loopback HTTP(S)"
        );
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            endpoint,
            token: token.into(),
        })
    }

    /// Envelope fields are kept out of `_metric` to preserve baseline enrichment keys.
    pub async fn emit(&self, metric: Value) -> Result<()> {
        let mut metric = metric
            .as_object()
            .cloned()
            .context("tail metric must be an object")?;
        let request = metric
            .remove("request")
            .context("tail metric request is required")?;
        anyhow::ensure!(request.is_object(), "tail request must be an object");
        if let Some(request_id) = metric.remove("request_id") {
            metric.insert("request".into(), request_id);
        }
        let response = metric.remove("response");
        let timestamp = metric.remove("eventTimestamp").unwrap_or_else(|| {
            json!(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        });
        let duration = metric.remove("wallTime").unwrap_or(json!(0));
        let mut event = json!({"request":request});
        if let Some(response) = response {
            event["response"] = response;
        }
        let traces = json!([{"event":event,"eventTimestamp":timestamp,"wallTime":duration,"logs":[{"message":[format!("_metric:{}",serde_json::to_string(&metric)?)]}]}]);
        // Error text intentionally excludes URL, headers and metric contents.
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.token)
            .json(&traces)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("native tail delivery failed"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "native tail ingress rejected delivery: HTTP {}",
            response.status().as_u16()
        );
        Ok(())
    }
}
