//! Purpose: typed LLM failure (`LLM.Error` with tagged reason union) preserving the
//! retryable/auth/quota/rate/overflow/content-policy/transport/cancel distinctions, plus
//! redaction-safe HTTP context.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/ids.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde::{Deserialize, Serialize}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- llm-api/src/client.rs
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/response.rs
//! `-- llm-api/src/route.rs
//! Invariants: `_tag` spellings and `retryable` per reason match
//! packages/llm/src/schema/errors.ts. `Cancelled` is Rust-only (Effect interruption in the
//! baseline) and is never retryable. Header values in `HttpContext` are already redacted by
//! the producer; this crate never stores raw credentials.
//! Contract tests: llm-api/tests/contract.rs

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::ids::{ModelId, ProviderId, ProviderMetadata, js_number};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderFailureClassification {
    #[serde(rename = "context-overflow")]
    ContextOverflow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpRequestDetails {
    pub method: String,
    pub url: String,
    pub headers: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpResponseDetails {
    pub status: u16,
    pub headers: IndexMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRateLimitDetails {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
    pub retry_after_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpContext {
    pub request: HttpRequestDetails,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<HttpResponseDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<HttpRateLimitDetails>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthenticationKind {
    Missing,
    Invalid,
    Expired,
    InsufficientPermissions,
    Unknown,
}

/// `LLMErrorReason` tagged union (`_tag`). Variant sizes differ because the baseline
/// attaches optional HTTP/rate-limit context; `LlmError` boxes the whole reason instead.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum LlmErrorReason {
    #[serde(rename_all = "camelCase")]
    InvalidRequest {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parameter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classification: Option<ProviderFailureClassification>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    NoRoute {
        route: String,
        provider: ProviderId,
        model: ModelId,
    },
    #[serde(rename_all = "camelCase")]
    Authentication {
        message: String,
        kind: AuthenticationKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    #[serde(rename_all = "camelCase")]
    RateLimit {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
        retry_after_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate_limit: Option<HttpRateLimitDetails>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    #[serde(rename_all = "camelCase")]
    QuotaExceeded {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    #[serde(rename_all = "camelCase")]
    ContentPolicy {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    #[serde(rename_all = "camelCase")]
    ProviderInternal {
        message: String,
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none", with = "js_number")]
        retry_after_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    Transport {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    #[serde(rename_all = "camelCase")]
    InvalidProviderOutput {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        route: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    #[serde(rename_all = "camelCase")]
    UnknownProvider {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<Box<HttpContext>>,
    },
    /// Rust-only: the caller's cancellation token fired (Effect interruption in the baseline).
    Cancelled { message: String },
}

impl LlmErrorReason {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "InvalidRequest",
            Self::NoRoute { .. } => "NoRoute",
            Self::Authentication { .. } => "Authentication",
            Self::RateLimit { .. } => "RateLimit",
            Self::QuotaExceeded { .. } => "QuotaExceeded",
            Self::ContentPolicy { .. } => "ContentPolicy",
            Self::ProviderInternal { .. } => "ProviderInternal",
            Self::Transport { .. } => "Transport",
            Self::InvalidProviderOutput { .. } => "InvalidProviderOutput",
            Self::UnknownProvider { .. } => "UnknownProvider",
            Self::Cancelled { .. } => "Cancelled",
        }
    }

    /// Only rate limits and provider-internal (5xx) failures are retryable.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::RateLimit { .. } | Self::ProviderInternal { .. })
    }

    pub fn retry_after_ms(&self) -> Option<f64> {
        match self {
            Self::RateLimit { retry_after_ms, .. }
            | Self::ProviderInternal { retry_after_ms, .. } => *retry_after_ms,
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoRoute {
                route,
                provider,
                model,
            } => format!("No LLM route for {provider}/{model} using {route}"),
            Self::InvalidRequest { message, .. }
            | Self::Authentication { message, .. }
            | Self::RateLimit { message, .. }
            | Self::QuotaExceeded { message, .. }
            | Self::ContentPolicy { message, .. }
            | Self::ProviderInternal { message, .. }
            | Self::Transport { message, .. }
            | Self::InvalidProviderOutput { message, .. }
            | Self::UnknownProvider { message, .. }
            | Self::Cancelled { message } => message.clone(),
        }
    }

    /// True for `InvalidRequest` classified as context overflow (compaction trigger).
    pub fn is_context_overflow(&self) -> bool {
        matches!(
            self,
            Self::InvalidRequest {
                classification: Some(ProviderFailureClassification::ContextOverflow),
                ..
            }
        )
    }
}

/// `LLM.Error`: `{ module, method, reason }`, message `"{module}.{method}: {reason.message}"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{module}.{method}: {}", reason.message())]
pub struct LlmError {
    pub module: String,
    pub method: String,
    pub reason: Box<LlmErrorReason>,
}

impl LlmError {
    pub fn new(
        module: impl Into<String>,
        method: impl Into<String>,
        reason: LlmErrorReason,
    ) -> Self {
        Self {
            module: module.into(),
            method: method.into(),
            reason: Box::new(reason),
        }
    }

    pub fn retryable(&self) -> bool {
        self.reason.retryable()
    }

    pub fn retry_after_ms(&self) -> Option<f64> {
        self.reason.retry_after_ms()
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(*self.reason, LlmErrorReason::Cancelled { .. })
    }

    /// `ProviderShared.eventError`: invalid provider output for `route`.
    pub fn invalid_output(
        route: impl Into<String>,
        message: impl Into<String>,
        raw: Option<String>,
    ) -> Self {
        Self::new(
            "ProviderShared",
            "stream",
            LlmErrorReason::InvalidProviderOutput {
                message: message.into(),
                route: Some(route.into()),
                raw,
                provider_metadata: None,
            },
        )
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(
            "ProviderShared",
            "request",
            LlmErrorReason::InvalidRequest {
                message: message.into(),
                parameter: None,
                classification: None,
                provider_metadata: None,
                http: None,
            },
        )
    }

    pub fn cancelled(module: impl Into<String>, method: impl Into<String>) -> Self {
        Self::new(
            module,
            method,
            LlmErrorReason::Cancelled {
                message: "Request cancelled".into(),
            },
        )
    }
}
