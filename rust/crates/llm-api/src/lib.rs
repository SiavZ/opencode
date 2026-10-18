//! Purpose: crate root for the provider-neutral LLM contract: canonical request, ordered
//! stream events, usage, typed errors and the route/client traits that adapters implement
//! and Session/hosted consumers call. Does not perform I/O.
//! Scope: pure values plus dyn-compatible traits. Portable (native + wasm32).
//! Uses:
//! rust/crates/
//! |-- llm-api/src/client.rs
//! |-- llm-api/src/error.rs
//! |-- llm-api/src/event.rs
//! |-- llm-api/src/ids.rs
//! |-- llm-api/src/message.rs
//! |-- llm-api/src/options.rs
//! |-- llm-api/src/response.rs
//! `-- llm-api/src/route.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, reserved contract (owner: phase-03, consumers: provider-adapters, session-api, session-runtime, tools, integration-api, hosted-adapters)
//! Invariants: wire shapes mirror packages/llm/src/schema JSON (camelCase keys, `type`/`_tag`
//! discriminators, optional keys omitted). No dependency on session, server or UI crates.
//! Contract tests: llm-api/tests/contract.rs

pub mod client;
pub mod error;
pub mod event;
pub mod ids;
pub mod message;
pub mod options;
pub mod response;
pub mod route;

pub use client::{LlmClient, RouteClient, generate};
pub use error::{
    AuthenticationKind, HttpContext, HttpRateLimitDetails, HttpRequestDetails, HttpResponseDetails,
    LlmError, LlmErrorReason, ProviderFailureClassification,
};
pub use event::{LlmEvent, Usage};
pub use ids::{
    FinishReason, JsonObject, MessageRole, ModelId, ProviderId, ProviderMetadata, ReasoningEffort,
    TextVerbosity,
};
pub use message::{
    ContentPart, LlmRequest, MediaData, Message, ResponseFormat, SystemPart, ToolCallPart,
    ToolChoice, ToolChoiceType, ToolContent, ToolDefinition, ToolOutput, ToolResultPart,
    ToolResultValue,
};
pub use options::{
    CacheHint, CacheHintType, CacheMessagesPolicy, CachePolicy, CachePolicyObject,
    GenerationOptions, HttpOptions, ModelCompatibility, ModelDefaults, ModelLimits,
    ProviderOptions, ToolSchemaCompatibility,
};
pub use response::{LlmResponse, ResponseState};
pub use route::{
    LlmEventStream, Model, ModelResolver, ModelSelection, PreparedRequest, ResolveError, Route,
    RouteDefaults, resolve_request_options,
};
