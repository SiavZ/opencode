//! Purpose: emit native Zen finalized metrics to the authenticated local tail processor.
//! Scope: request-owned metadata, usage and completion timing; never prompt or credential logs.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/workers/producer.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/gateway/usage.rs
//! External:
//! |-- http::request::Parts
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/zen/completion.rs
//! `-- hosted/src/console/zen/legacy.rs
//! Invariants: emission is awaited and bounded, contains no body or credentials, and failures do not charge twice.
//! Contract tests: hosted/src/console/zen/acceptance.rs

use super::super::workers::producer::TailEmitter;
use http::request::Parts;
use opencode_hosted_adapters::console::{
    formats::UsageInfo,
    gateway::usage::{BillingContext, UsageRecord},
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct GatewayTrace {
    emitter: Option<TailEmitter>,
    pub metric: Value,
    started: i64,
}

impl GatewayTrace {
    pub fn new(parts: &Parts, emitter: Option<TailEmitter>) -> Self {
        let header = |name| {
            parts
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
        };
        let started = timestamp();
        let host = header("host");
        // HTTP/2 carries scheme and authority in the URI rather than a Host header.
        // Userinfo is never telemetry, including in a supplied HTTP/1 Host header.
        let scheme = parts
            .uri
            .scheme_str()
            .filter(|scheme| matches!(*scheme, "http" | "https"))
            .unwrap_or("http");
        let authority = parts
            .uri
            .authority()
            .cloned()
            .or_else(|| host.parse::<http::uri::Authority>().ok());
        let authority = authority
            .as_ref()
            .map(|authority| authority.as_str().rsplit('@').next().unwrap_or("localhost"))
            .unwrap_or("localhost");
        let path = parts.uri.path_and_query().map_or("/", |path| path.as_str());
        let request_url = format!("{scheme}://{authority}{path}");
        Self {
            emitter,
            started,
            metric: json!({
                "request":{"method":parts.method.as_str(),"url":request_url,"headers":{"x-real-ip":header("x-real-ip"),"content-length":header("content-length")},"cf":{"country":header("cf-ipcountry")}},
                "eventTimestamp":started,"session":header("x-opencode-session"),"request_id":header("x-opencode-request"),"client":header("x-opencode-client"),"user_agent":header("user-agent"),"model.tier":if parts.uri.path().starts_with("/zen/go/"){"go"}else{"zen"}
            }),
        }
    }
    pub fn identity(
        &mut self,
        context: &BillingContext,
        model: &str,
        provider: &str,
        native: &str,
    ) {
        self.metric["workspace"] = json!(context.workspace_id);
        self.metric["api_key"] = json!(context.key_id());
        self.metric["user_id"] = json!(context.user_id());
        self.metric["source"] = json!(context.source.as_str());
        if let Some(plan) = context.subscription_plan() {
            self.metric["subscription"] = json!(plan);
        }
        self.metric["model"] = json!(model);
        self.metric["provider"] = json!(provider);
        self.metric["provider.model"] = json!(native);
        self.metric["shallowProvider"] = json!(provider);
        self.metric["shallowProvider.model"] = json!(native);
    }
    pub fn usage(
        &mut self,
        model: &Value,
        usage: &UsageInfo,
        record: &UsageRecord,
    ) -> Result<(), opencode_hosted_adapters::console::gateway::usage::UsageError> {
        for (name, value) in [
            ("tokens.input", Some(usage.input_tokens)),
            ("tokens.output", Some(usage.output_tokens)),
            ("tokens.reasoning", usage.reasoning_tokens),
            ("tokens.cache_read", usage.cache_read_tokens),
            ("tokens.cache_write_5m", usage.cache_write5m_tokens),
            ("tokens.cache_write_1h", usage.cache_write1h_tokens),
        ] {
            if let Some(value) = value {
                self.metric[name] = json!(value);
            }
        }
        let mut components =
            opencode_hosted_adapters::console::gateway::usage::calculate_cost_components(
                model,
                usage,
                record.tracked_at,
            )?;
        let components = components.as_object_mut().expect("component metric object");
        components.remove("rawCents");
        self.metric
            .as_object_mut()
            .expect("trace metric object")
            .extend(components.clone());
        Ok(())
    }
    pub async fn finish(
        mut self,
        status: u16,
        response_length: usize,
        first_byte: i64,
        stream: bool,
    ) {
        self.metric["response"] = json!({"status":status});
        self.metric["wallTime"] = json!(timestamp() - self.started);
        self.metric["response_length"] = json!(response_length);
        self.metric["is_stream"] = json!(stream);
        if first_byte > 0 {
            self.metric["timestamp.first_byte"] = json!(first_byte);
            self.metric["timestamp.last_byte"] = json!(timestamp());
            self.metric["time_to_first_byte"] = json!(first_byte - self.started);
        }
        if let Some(emitter) = self.emitter
            && let Err(error) = emitter.emit(self.metric).await
        {
            tracing::warn!(error=%error,"Gateway tail delivery failed");
        }
    }
}
pub fn timestamp() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}
