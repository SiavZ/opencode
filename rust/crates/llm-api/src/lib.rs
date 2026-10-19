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
//! rust/crates/
//! |-- llm-api/tests/contract.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/cache_policy.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/anthropic/body.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/anthropic/stream.rs
//! |-- provider-adapters/src/protocols/anthropic/wire.rs
//! |-- provider-adapters/src/protocols/bedrock/auth.rs
//! |-- provider-adapters/src/protocols/bedrock/body.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/protocols/bedrock/stream.rs
//! |-- provider-adapters/src/protocols/gemini/body.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/protocols/gemini/stream.rs
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/protocols/openai/chat_stream.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/protocols/openai/options.rs
//! |-- provider-adapters/src/protocols/openai/responses_body.rs
//! |-- provider-adapters/src/protocols/openai/responses_events.rs
//! |-- provider-adapters/src/protocols/openai/responses_stream.rs
//! |-- provider-adapters/src/protocols/openai/wire.rs
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/gitlab_dws_discovery.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/openai.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! |-- provider-adapters/src/providers/transform.rs
//! |-- provider-adapters/src/providers/wrap.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/shared.rs
//! |-- provider-adapters/src/shared/media.rs
//! |-- provider-adapters/src/tool_schema.rs
//! |-- provider-adapters/src/transport/event_stream.rs
//! |-- provider-adapters/src/transport/executor.rs
//! |-- provider-adapters/src/transport/http.rs
//! |-- provider-adapters/src/transport/status.rs
//! |-- provider-adapters/src/transport/websocket.rs
//! |-- provider-adapters/src/unsupported.rs
//! |-- provider-adapters/tests/anthropic_messages.rs
//! |-- provider-adapters/tests/bedrock_converse.rs
//! |-- provider-adapters/tests/gemini.rs
//! |-- provider-adapters/tests/openai_chat.rs
//! |-- provider-adapters/tests/openai_common/mod.rs
//! |-- provider-adapters/tests/openai_recorded.rs
//! |-- provider-adapters/tests/openai_responses.rs
//! |-- provider-adapters/tests/providers_cloud.rs
//! |-- provider-adapters/tests/providers_facades.rs
//! |-- provider-adapters/tests/providers_gateways.rs
//! |-- provider-adapters/tests/providers_registry.rs
//! |-- provider-adapters/tests/providers_support/mod.rs
//! |-- provider-adapters/tests/providers_transform.rs
//! |-- provider-adapters/tests/providers_transform_messages.rs
//! |-- provider-adapters/tests/transport_executor.rs
//! |-- provider-adapters/tests/transport_framing.rs
//! |-- provider-adapters/tests/transport_http.rs
//! |-- provider-adapters/tests/transport_websocket.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_routes.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/runner/max_steps.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/to_llm_message.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/runner/turn/request.rs
//! |-- session-runtime/src/runner/turn/tools.rs
//! |-- session-runtime/src/title.rs
//! |-- session-runtime/tests/compaction.rs
//! |-- session-runtime/tests/runner_messages.rs
//! |-- session-runtime/tests/runner_model.rs
//! |-- session-runtime/tests/runner_turn_steps.rs
//! |-- session-runtime/tests/runner_turn_tools.rs
//! |-- session-runtime/tests/support/model.rs
//! |-- session-runtime/tests/support/provider.rs
//! `-- session-runtime/tests/title.rs
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
