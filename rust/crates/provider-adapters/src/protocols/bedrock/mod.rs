//! Purpose: Amazon Bedrock Converse protocol (packages/llm/src/protocols/bedrock-converse.ts):
//! the `Protocol` impl, SigV4 auth, and the default route constructor
//! (`/model/{encodeURIComponent(modelId)}/converse-stream`, AWS event-stream framing).
//! Scope: module root.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocol.rs
//! |-- provider-adapters/src/protocols/bedrock/auth.rs
//! |-- provider-adapters/src/protocols/bedrock/body.rs
//! |-- provider-adapters/src/protocols/bedrock/stream.rs
//! |-- provider-adapters/src/route.rs
//! `-- provider-adapters/src/transport/redact.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/mod.rs
//! `-- provider-adapters/tests/bedrock_converse.rs
//! Invariants: owned by phase-03 leaf p03.messages; the URL path is computed from the built body's
//! `modelId` so it matches the signed bytes; default auth is SigV4 without credentials (fails
//! with a typed error until a provider configures credentials or bearer auth).
//! Contract tests: provider-adapters/tests/bedrock_converse.rs

mod auth;
mod body;
mod stream;

use std::sync::Arc;

use opencode_llm_api::{LlmError, LlmRequest, ProviderId};
use serde_json::Value;

pub use auth::{BedrockCredentials, SigV4Auth, sign_headers};

use crate::protocol::{Protocol, ProtocolState};
use crate::route::{Endpoint, EndpointPath, ProtocolRoute, Transport};
use crate::transport::redact::encode_uri_component;

pub const ID: &str = "bedrock-converse";

/// `https://bedrock-runtime.<region>.amazonaws.com`.
pub fn base_url(region: &str) -> String {
    format!("https://bedrock-runtime.{region}.amazonaws.com")
}

/// `/model/{encodeURIComponent(modelId)}/converse-stream`.
pub fn path(model_id: &str) -> String {
    format!("/model/{}/converse-stream", encode_uri_component(model_id))
}

/// The Bedrock Converse protocol: body lowering plus the event-stream state machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct BedrockConverse;

impl Protocol for BedrockConverse {
    fn id(&self) -> &'static str {
        ID
    }

    fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        body::body(request)
    }

    fn initial(&self, _request: &LlmRequest) -> Box<dyn ProtocolState> {
        Box::new(stream::State::default())
    }
}

/// `BedrockConverse.route`: provider `bedrock`, no base URL (providers set
/// `base_url(region)`), SigV4 auth from `credentials`. `transport` must use
/// `Framing::aws_event_stream("bedrock-converse")`.
pub fn route(
    transport: Arc<dyn Transport>,
    credentials: Option<BedrockCredentials>,
) -> ProtocolRoute {
    let mut route = ProtocolRoute::new(
        ID,
        Arc::new(BedrockConverse),
        Endpoint {
            base_url: credentials.as_ref().map(|c| base_url(&c.region)),
            path: EndpointPath::Dynamic(Arc::new(|request, body| {
                path(
                    body.get("modelId")
                        .and_then(Value::as_str)
                        .unwrap_or(request.model.id.as_str()),
                )
            })),
            query: Default::default(),
        },
        transport,
    );
    route.provider = Some(ProviderId::new("bedrock"));
    route.auth = Arc::new(SigV4Auth::new(credentials));
    route
}
