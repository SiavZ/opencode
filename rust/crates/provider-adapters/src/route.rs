//! Purpose: compose Protocol + Endpoint + Auth + Transport into a runnable `opencode_llm_api::Route`
//! (packages/llm/src/route/client.ts `Route.make`, `compile`, endpoint.ts, transport/http.ts
//! `jsonRequestParts`). Owns cache-policy application, body overlay protection and header order.
//! Scope: request compilation and stream wiring. Network I/O is delegated to `Transport`.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- llm-api/src/options.rs
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/cache_policy.rs
//! `-- provider-adapters/src/protocol.rs
//! External:
//! |-- async_stream::try_stream
//! |-- fmt::Debug
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- futures::StreamExt
//! |-- futures::stream::BoxStream
//! |-- futures::stream::iter
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- serde_json::to_string
//! |-- str::to_owned
//! |-- tokio::test
//! |-- url::Url
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/auth.rs
//! |-- provider-adapters/src/cache_policy.rs
//! |-- provider-adapters/src/lib.rs
//! |-- provider-adapters/src/protocols/anthropic/mod.rs
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! |-- provider-adapters/src/protocols/gemini/mod.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/openai.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! |-- provider-adapters/src/transport/executor.rs
//! |-- provider-adapters/src/transport/http.rs
//! |-- provider-adapters/src/transport/websocket.rs
//! |-- provider-adapters/tests/anthropic_messages.rs
//! |-- provider-adapters/tests/bedrock_converse.rs
//! |-- provider-adapters/tests/gemini.rs
//! |-- provider-adapters/tests/openai_chat.rs
//! |-- provider-adapters/tests/openai_common/mod.rs
//! |-- provider-adapters/tests/openai_recorded.rs
//! |-- provider-adapters/tests/openai_responses.rs
//! |-- provider-adapters/tests/transport_executor.rs
//! |-- provider-adapters/tests/transport_http.rs
//! |-- provider-adapters/tests/transport_websocket.rs
//! `-- runtime/src/execution_support.rs
//! Invariants: one `stream` call compiles once and opens exactly one transport request; status
//! retries live in the transport executor before the first frame. `http.body` may not overlay
//! protocol-owned keys. Dropping the stream drops the transport future (cancellation).
//! Contract tests: provider-adapters/src/route.rs (unit tests)

use std::fmt;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use opencode_llm_api::options::merge_json_records;
use opencode_llm_api::{
    LlmError, LlmEvent, LlmEventStream, LlmRequest, PreparedRequest, ProviderId, Route,
    RouteDefaults,
};
use serde_json::Value;

use crate::auth::{AuthInput, Headers, NoAuth, SharedAuth};
use crate::cache_policy::apply_cache_policy;
use crate::protocol::{Frame, Protocol};

/// Ordered frames for one response.
pub type FrameStream = BoxStream<'static, Result<Frame, LlmError>>;

/// Fully prepared outgoing request (URL, final headers including auth, exact body bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequestParts {
    pub method: String,
    pub url: String,
    pub headers: Headers,
    pub body: String,
    /// Body as JSON after overlay (WebSocket transports re-wrap this).
    pub json_body: Value,
}

/// Executes a prepared request and yields protocol frames (`http-json` + framing, `websocket-json`).
pub trait Transport: Send + Sync + fmt::Debug {
    fn id(&self) -> &str;
    /// `route_label` is `"{provider}/{route_id}"` for diagnostics.
    fn frames(&self, request: HttpRequestParts, route_label: String) -> FrameStream;
}

/// Path computed from the request and built body (Bedrock and Gemini embed the model id).
pub type PathFn = Arc<dyn Fn(&LlmRequest, &Value) -> String + Send + Sync>;

#[derive(Clone)]
pub enum EndpointPath {
    Static(String),
    Dynamic(PathFn),
}

impl fmt::Debug for EndpointPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(p) => write!(f, "Static({p:?})"),
            Self::Dynamic(_) => f.write_str("Dynamic(..)"),
        }
    }
}

/// `Endpoint`: base URL + path + static query.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub base_url: Option<String>,
    pub path: EndpointPath,
    pub query: Headers,
}

impl Endpoint {
    pub fn new(base_url: Option<&str>, path: &str) -> Self {
        Self {
            base_url: base_url.map(str::to_owned),
            path: EndpointPath::Static(path.to_owned()),
            query: Headers::new(),
        }
    }

    /// `Endpoint.render` plus `applyQuery(request.http.query)`.
    pub fn render(&self, request: &LlmRequest, body: &Value) -> Result<String, LlmError> {
        let path = match &self.path {
            EndpointPath::Static(p) => p.clone(),
            EndpointPath::Dynamic(f) => f(request, body),
        };
        let base = trim_base_url(self.base_url.as_deref().unwrap_or(""));
        let mut url = url::Url::parse(&format!("{base}{path}"))
            .map_err(|e| LlmError::invalid_request(format!("Invalid endpoint URL: {e}")))?;
        let extra = request.http.as_ref().and_then(|h| h.query.as_ref());
        for (k, v) in self.query.iter().chain(extra.into_iter().flatten()) {
            set_query(&mut url, k, v);
        }
        Ok(url.to_string())
    }
}

/// `URLSearchParams.set`: replace all existing values of `key`.
fn set_query(url: &mut url::Url, key: &str, value: &str) {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != key)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut pairs = url.query_pairs_mut();
    pairs.clear();
    for (k, v) in kept {
        pairs.append_pair(&k, &v);
    }
    pairs.append_pair(key, value);
}

pub fn trim_base_url(value: &str) -> &str {
    value.trim_end_matches('/')
}

/// Keys of the provider body that `http.body` may never overlay (transport/http.ts).
pub const PROTOCOL_BODY_OVERLAY_DENYLIST: &[&str] = &[
    "content",
    "contents",
    "frequencyPenalty",
    "frequency_penalty",
    "generationConfig",
    "inferenceConfig",
    "input",
    "maxTokens",
    "max_tokens",
    "messages",
    "model",
    "presencePenalty",
    "presence_penalty",
    "responseFormat",
    "response_format",
    "seed",
    "stop",
    "stopSequences",
    "stop_sequences",
    "stream",
    "streamOptions",
    "stream_options",
    "system",
    "systemInstruction",
    "system_instruction",
    "temperature",
    "thinking",
    "toolChoice",
    "toolConfig",
    "tool_choice",
    "tool_config",
    "tools",
    "topK",
    "topP",
    "top_k",
    "top_p",
];

/// Static or request-dependent headers added before auth runs.
pub type HeadersFn = Arc<dyn Fn(&LlmRequest) -> Headers + Send + Sync>;

/// A runnable deployment of one protocol.
#[derive(Clone)]
pub struct ProtocolRoute {
    pub id: String,
    pub provider: Option<ProviderId>,
    pub protocol: Arc<dyn Protocol>,
    pub endpoint: Endpoint,
    pub auth: SharedAuth,
    pub headers: Option<HeadersFn>,
    pub transport: Arc<dyn Transport>,
    pub defaults: RouteDefaults,
}

impl fmt::Debug for ProtocolRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolRoute")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("protocol", &self.protocol.id())
            .field("endpoint", &self.endpoint)
            .field("auth", &self.auth)
            .field("transport", &self.transport.id())
            .finish()
    }
}

impl ProtocolRoute {
    pub fn new(
        id: impl Into<String>,
        protocol: Arc<dyn Protocol>,
        endpoint: Endpoint,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            id: id.into(),
            provider: None,
            protocol,
            endpoint,
            auth: Arc::new(NoAuth),
            headers: None,
            transport,
            defaults: RouteDefaults::default(),
        }
    }

    /// `compile`: cache policy, then protocol body. The request is already option-resolved.
    pub fn compile(&self, request: &LlmRequest) -> Result<(LlmRequest, Value), LlmError> {
        let request = apply_cache_policy(&self.id, request.clone());
        let body = self.protocol.body(&request)?;
        Ok((request, body))
    }

    /// `jsonRequestParts`: URL, overlay, headers (route < request.http.headers), auth, content-type.
    pub async fn prepare_http(
        &self,
        request: &LlmRequest,
        body: Value,
    ) -> Result<HttpRequestParts, LlmError> {
        let url = self.endpoint.render(request, &body)?;
        let json_body = overlay_body(body, request)?;
        let body_text = serde_json::to_string(&json_body)
            .map_err(|e| LlmError::invalid_request(e.to_string()))?;
        let mut headers = Headers::new();
        if let Some(f) = &self.headers {
            for (k, v) in f(request) {
                headers.insert(k.to_ascii_lowercase(), v);
            }
        }
        if let Some(extra) = request.http.as_ref().and_then(|h| h.headers.as_ref()) {
            for (k, v) in extra {
                headers.insert(k.to_ascii_lowercase(), v.clone());
            }
        }
        let input = AuthInput {
            request,
            method: "POST",
            url: &url,
            body: &body_text,
        };
        let mut headers = self.auth.apply(input, headers).await?;
        headers.insert("content-type".into(), "application/json".into());
        Ok(HttpRequestParts {
            method: "POST".into(),
            url,
            headers,
            body: body_text,
            json_body,
        })
    }

    fn label(&self, request: &LlmRequest) -> String {
        format!("{}/{}", request.model.provider, self.id)
    }
}

fn overlay_body(body: Value, request: &LlmRequest) -> Result<Value, LlmError> {
    let Some(overlay) = request.http.as_ref().and_then(|h| h.body.as_ref()) else {
        return Ok(body);
    };
    let forbidden: Vec<&str> = overlay
        .keys()
        .filter(|k| PROTOCOL_BODY_OVERLAY_DENYLIST.contains(&k.as_str()))
        .map(String::as_str)
        .collect();
    if !forbidden.is_empty() {
        return Err(LlmError::invalid_request(format!(
            "http.body cannot overlay protocol-owned field(s): {}",
            forbidden.join(", ")
        )));
    }
    match body {
        Value::Object(map) => Ok(Value::Object(
            merge_json_records([Some(&map), Some(overlay)]).unwrap_or_default(),
        )),
        _ => Err(LlmError::invalid_request(
            "http.body can only overlay JSON object request bodies",
        )),
    }
}

impl Route for ProtocolRoute {
    fn id(&self) -> &str {
        &self.id
    }

    fn protocol(&self) -> &str {
        self.protocol.id()
    }

    fn transport(&self) -> &str {
        self.transport.id()
    }

    fn provider(&self) -> Option<&ProviderId> {
        self.provider.as_ref()
    }

    fn defaults(&self) -> &RouteDefaults {
        &self.defaults
    }

    fn prepare(&self, request: &LlmRequest) -> Result<PreparedRequest, LlmError> {
        let (request, body) = self.compile(request)?;
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "transport".into(),
            Value::String(self.transport.id().to_owned()),
        );
        Ok(PreparedRequest {
            id: request.id.clone().unwrap_or_else(|| "request".into()),
            route: self.id.clone(),
            protocol: self.protocol.id().to_owned(),
            model: request.model.id.clone(),
            provider: request.model.provider.clone(),
            body,
            metadata: Some(metadata),
        })
    }

    fn stream(&self, request: LlmRequest) -> LlmEventStream {
        let route = self.clone();
        let stream = async_stream::try_stream! {
            let label = route.label(&request);
            let (request, body) = route.compile(&request)?;
            let parts = route.prepare_http(&request, body).await?;
            let mut state = route.protocol.initial(&request);
            let mut frames = route.transport.frames(parts, label.clone());
            let mut terminated = false;
            while let Some(frame) = frames.next().await {
                let events: Vec<LlmEvent> = state.step(&label, frame?)?;
                for event in events {
                    yield event;
                }
                if state.terminal() {
                    terminated = true;
                    break;
                }
            }
            if !terminated {
                for event in state.on_halt(&label)? {
                    yield event;
                }
            }
        };
        Box::pin(stream)
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    #[derive(Debug)]
    pub struct EchoProtocol;

    struct EchoState;

    impl crate::protocol::ProtocolState for EchoState {
        fn step(&mut self, _route: &str, frame: Frame) -> Result<Vec<LlmEvent>, LlmError> {
            Ok(vec![LlmEvent::TextDelta {
                id: "t".into(),
                text: frame.raw(),
                provider_metadata: None,
            }])
        }

        fn on_halt(&mut self, _route: &str) -> Result<Vec<LlmEvent>, LlmError> {
            Ok(vec![LlmEvent::Finish {
                reason: opencode_llm_api::FinishReason::Stop,
                usage: None,
                provider_metadata: None,
            }])
        }
    }

    impl Protocol for EchoProtocol {
        fn id(&self) -> &'static str {
            "echo"
        }

        fn body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
            Ok(serde_json::json!({ "model": request.model.id.as_str(), "stream": true }))
        }

        fn initial(&self, _request: &LlmRequest) -> Box<dyn crate::protocol::ProtocolState> {
            Box::new(EchoState)
        }
    }

    /// Test-only transport that replays fixed frames and records the request it was given.
    #[derive(Debug, Default)]
    pub struct FixedFrames {
        pub frames: Vec<String>,
        pub seen: std::sync::Mutex<Vec<HttpRequestParts>>,
    }

    impl Transport for FixedFrames {
        fn id(&self) -> &str {
            "fixed"
        }

        fn frames(&self, request: HttpRequestParts, _route_label: String) -> FrameStream {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request);
            let frames: Vec<Result<Frame, LlmError>> = self
                .frames
                .iter()
                .cloned()
                .map(|f| Ok(Frame::Text(f)))
                .collect();
            Box::pin(futures::stream::iter(frames))
        }
    }

    pub fn dummy_route() -> Arc<dyn Route> {
        Arc::new(ProtocolRoute::new(
            "echo",
            Arc::new(EchoProtocol),
            Endpoint::new(Some("https://example.test/v1/"), "/echo"),
            Arc::new(FixedFrames::default()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use opencode_llm_api::{HttpOptions, Model, RouteClient, generate};

    fn route(frames: &[&str]) -> (Arc<ProtocolRoute>, Arc<FixedFrames>) {
        let transport = Arc::new(FixedFrames {
            frames: frames.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        });
        let mut route = ProtocolRoute::new(
            "echo",
            Arc::new(EchoProtocol),
            Endpoint::new(Some("https://example.test/v1/"), "/echo"),
            transport.clone(),
        );
        route.auth = crate::auth::bearer_value("sk-test");
        route.headers = Some(Arc::new(|_| {
            Headers::from([("X-Route".into(), "r".into())])
        }));
        (Arc::new(route), transport)
    }

    #[tokio::test]
    async fn stream_compiles_once_and_flushes_on_halt() {
        let (route, transport) = route(&["a", "b"]);
        let mut request = LlmRequest::new(Model::new("m1", "prov", route));
        request.http = Some(HttpOptions {
            body: Some(serde_json::from_str(r#"{"user":"u"}"#).unwrap()),
            headers: Some(
                [("x-req".to_string(), "q".to_string())]
                    .into_iter()
                    .collect(),
            ),
            query: Some(
                [("api-version".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
            ),
        });
        let response = generate(&RouteClient, request).await.unwrap();
        assert_eq!(response.text(), "ab");
        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "https://example.test/v1/echo?api-version=1");
        assert_eq!(seen[0].body, r#"{"model":"m1","stream":true,"user":"u"}"#);
        let keys: Vec<&str> = seen[0].headers.keys().map(String::as_str).collect();
        assert_eq!(keys, ["x-route", "x-req", "authorization", "content-type"]);
    }

    #[tokio::test]
    async fn overlay_of_protocol_fields_is_rejected() {
        let (route, _) = route(&[]);
        let mut request = LlmRequest::new(Model::new("m1", "prov", route.clone()));
        request.http = Some(HttpOptions {
            body: Some(serde_json::from_str(r#"{"model":"x","messages":[]}"#).unwrap()),
            headers: None,
            query: None,
        });
        let (request, body) = route.compile(&request).unwrap();
        let err = route.prepare_http(&request, body).await.unwrap_err();
        assert_eq!(
            err.reason.message(),
            "http.body cannot overlay protocol-owned field(s): model, messages"
        );
    }
}
