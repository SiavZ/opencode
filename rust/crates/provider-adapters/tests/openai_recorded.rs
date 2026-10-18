//! Purpose: replay every recorded OpenAI cassette (openai-chat, openai-compatible-chat,
//! openai-responses, openai-responses-cache): rebuild each scenario's request in Rust, assert the
//! request URL and body match the recording (byte-equal where the recording used today's key
//! order, JSON-equal otherwise), and assert the decoded event stream equals the event stream
//! the TypeScript protocol produced for the same recorded response (tests/fixtures/openai-golden).
//! Scope: fixture-based (recorded cassettes), no live provider calls.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/protocols/openai/mod.rs
//! `-- provider-adapters/tests/openai_common/mod.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: multi-turn cassettes are driven like packages/llm/test/recorded-scenarios.ts: the
//! next request is built from the previous Rust response, so continuation lowering is covered.
//! Golden events were captured from packages/llm with `LLMClient.stream` over each recorded body.
//! Contract tests: provider-adapters/tests/openai_recorded.rs

mod openai_common;

use std::sync::Arc;

use openai_common::*;
use opencode_llm_api::{
    ContentPart, LlmEvent, LlmRequest, LlmResponse, Message, MessageRole, RouteDefaults,
    ToolCallPart, ToolChoice, ToolContent, ToolDefinition, ToolResultPart, ToolResultValue,
};
use opencode_provider_adapters::protocols::openai::{
    chat_route, compatible_chat_route, responses_route,
};
use opencode_provider_adapters::route::ProtocolRoute;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Chat,
    Compatible,
    Responses,
}

#[derive(Clone, Copy, PartialEq)]
enum BodyMatch {
    Bytes,
    Json,
}

fn golden(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/openai-golden/events.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    all[name]
        .as_array()
        .unwrap_or_else(|| panic!("no golden for {name}"))
        .iter()
        .map(|run| run["events"].clone())
        .collect()
}

fn route_for(family: Family, url: &str, replay: Arc<Replay>, keep_defaults: bool) -> ProtocolRoute {
    let mut route = match family {
        Family::Chat => chat_route(replay),
        Family::Compatible => {
            let mut route = compatible_chat_route(replay);
            let base = url.strip_suffix("/chat/completions").unwrap();
            route.endpoint.base_url = Some(base.to_owned());
            route
        }
        Family::Responses => responses_route(replay),
    };
    if !keep_defaults {
        route.defaults = RouteDefaults::default();
    }
    route
}

/// Runs one cassette: `build(turn, previous)` produces each turn's request.
async fn run_cassette(
    name: &str,
    family: Family,
    model_id: &str,
    keep_defaults: bool,
    body_match: BodyMatch,
    mut build: impl FnMut(usize, LlmRequest, Option<&(LlmRequest, LlmResponse)>) -> LlmRequest,
) -> Vec<LlmResponse> {
    let interactions = cassette(name);
    let goldens = golden(name);
    let replay = Replay::new(interactions.iter().map(|i| i.response_body.clone()));
    let route = route_for(family, &interactions[0].url, replay.clone(), keep_defaults);
    let model = model(route, model_id, "openai");
    let mut previous: Option<(LlmRequest, LlmResponse)> = None;
    let mut responses = Vec::new();
    for (turn, interaction) in interactions.iter().enumerate() {
        let request = build(turn, LlmRequest::new(model.clone()), previous.as_ref());
        let response = generate(request.clone())
            .await
            .unwrap_or_else(|e| panic!("{name} turn {turn}: {e}"));
        let sent = replay.seen.lock().unwrap()[turn].clone();
        assert_eq!(sent.url, interaction.url, "{name} turn {turn} url");
        match body_match {
            BodyMatch::Bytes => assert_eq!(
                sent.body, interaction.request_body,
                "{name} turn {turn} body"
            ),
            BodyMatch::Json => {
                let recorded: Value = serde_json::from_str(&interaction.request_body).unwrap();
                assert_eq!(sent.json_body, recorded, "{name} turn {turn} body");
            }
        }
        assert_eq!(
            Value::Array(to_json(&response.events)),
            goldens[turn],
            "{name} turn {turn} events"
        );
        responses.push(response.clone());
        previous = Some((request, response));
    }
    responses
}

fn user(text: &str) -> Message {
    Message::user(text)
}

/// `assistantContent` from recorded-scenarios.ts `runWeatherToolLoop`.
fn assistant_content(events: &[LlmEvent]) -> Vec<ContentPart> {
    let mut content: Vec<ContentPart> = Vec::new();
    for event in events {
        match event {
            LlmEvent::TextDelta { text, .. } => match content.last_mut() {
                Some(ContentPart::Text { text: last, .. }) => last.push_str(text),
                _ => content.push(ContentPart::text(text.clone())),
            },
            LlmEvent::ReasoningDelta { text, .. } => match content.last_mut() {
                Some(ContentPart::Reasoning { text: last, .. }) => last.push_str(text),
                _ => content.push(ContentPart::reasoning(text.clone())),
            },
            LlmEvent::ToolCall {
                id,
                name,
                input,
                provider_executed,
                provider_metadata,
            } => content.push(ContentPart::ToolCall(ToolCallPart {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                provider_executed: *provider_executed,
                metadata: None,
                provider_metadata: provider_metadata.clone(),
            })),
            _ => {}
        }
    }
    content
}

/// Next tool-loop turn: previous messages + assistant turn + the weather tool result.
fn tool_loop_next(prev: &(LlmRequest, LlmResponse)) -> Vec<Message> {
    let (request, response) = prev;
    let mut messages = request.messages.clone();
    messages.push(Message::assistant(assistant_content(&response.events)));
    for call in response.tool_calls() {
        let LlmEvent::ToolCall { id, name, .. } = call else {
            continue;
        };
        messages.push(Message::tool(ToolResultPart {
            id: id.clone(),
            name: name.clone(),
            result: ToolResultValue::Json(json!({ "temperature": 22, "condition": "sunny" })),
            provider_executed: None,
            cache: None,
            metadata: None,
            provider_metadata: None,
        }));
    }
    messages
}

fn tool_loop(
    system_text: &'static str,
    temperature: Option<f64>,
) -> impl FnMut(usize, LlmRequest, Option<&(LlmRequest, LlmResponse)>) -> LlmRequest {
    move |_, mut request, prev| {
        request.system = system(system_text);
        request.tools = vec![weather_tool()];
        request.generation = Some(generation(80.0, temperature));
        request.messages = match prev {
            None => vec![user("What is the weather in Paris?")],
            Some(prev) => tool_loop_next(prev),
        };
        request
    }
}

fn text_turn(
    system_text: &'static str,
    prompt: &'static str,
    max: f64,
    temperature: Option<f64>,
) -> impl FnMut(usize, LlmRequest, Option<&(LlmRequest, LlmResponse)>) -> LlmRequest {
    move |_, mut request, _| {
        request.system = system(system_text);
        request.messages = vec![user(prompt)];
        request.generation = Some(generation(max, temperature));
        request
    }
}

fn tool_call_turn(
    temperature: Option<f64>,
) -> impl FnMut(usize, LlmRequest, Option<&(LlmRequest, LlmResponse)>) -> LlmRequest {
    move |_, mut request, _| {
        request.system = system("Call tools exactly as requested.");
        request.messages = vec![user("Call get_weather with city exactly Paris.")];
        request.tools = vec![weather_tool()];
        request.tool_choice = Some(ToolChoice::named("get_weather"));
        request.generation = Some(generation(80.0, temperature));
        request
    }
}

const LOOP: &str = "Use the get_weather tool, then answer in one short sentence.";
const LOOP_ONCE: &str = "Use the get_weather tool exactly once, then answer in one short sentence.";

#[tokio::test]
async fn openai_chat_cassettes() {
    let t = Some(0.0);
    let r = run_cassette(
        "openai-chat/streams-text",
        Family::Chat,
        "gpt-4o-mini",
        true,
        BodyMatch::Bytes,
        text_turn(
            "You are concise.",
            "Say hello in one short sentence.",
            20.0,
            t,
        ),
    )
    .await;
    assert_eq!(r[0].text(), "Hello!");
    let r = run_cassette(
        "openai-chat/streams-tool-call",
        Family::Chat,
        "gpt-4o-mini",
        true,
        BodyMatch::Bytes,
        tool_call_turn(t),
    )
    .await;
    assert_eq!(r[0].tool_calls().count(), 1);
    assert_eq!(
        r[0].finish_reason,
        opencode_llm_api::FinishReason::ToolCalls
    );
    let r = run_cassette(
        "openai-chat/drives-a-tool-loop-end-to-end",
        Family::Chat,
        "gpt-4o-mini",
        true,
        BodyMatch::Bytes,
        tool_loop(LOOP, t),
    )
    .await;
    assert!(r[1].text().contains("Paris"));
    run_cassette(
        "openai-chat/continues-after-tool-result",
        Family::Chat,
        "gpt-4o-mini",
        true,
        BodyMatch::Bytes,
        |_, mut request, _| {
            request.system = system("Answer using only the provided tool result.");
            request.generation = Some(generation(40.0, Some(0.0)));
            request.messages = vec![
                user("What is the weather in Paris?"),
                Message::assistant(vec![ContentPart::ToolCall(ToolCallPart {
                    id: "call_weather".into(),
                    name: "get_weather".into(),
                    input: json!({ "city": "Paris" }),
                    provider_executed: None,
                    metadata: None,
                    provider_metadata: None,
                })]),
                Message::tool(ToolResultPart {
                    id: "call_weather".into(),
                    name: "get_weather".into(),
                    result: ToolResultValue::Json(
                        json!({ "forecast": "sunny", "temperature_c": 22 }),
                    ),
                    provider_executed: None,
                    cache: None,
                    metadata: None,
                    provider_metadata: None,
                }),
            ];
            request
        },
    )
    .await;
}

#[tokio::test]
async fn openai_compatible_chat_cassettes() {
    let t = Some(0.0);
    let hello = |name, model| (name, model);
    for (name, model) in [
        hello("deepseek-streams-text", "deepseek-chat"),
        hello("groq-streams-text", "llama-3.3-70b-versatile"),
        hello("openrouter-streams-text", "openai/gpt-4o-mini"),
        hello(
            "togetherai-streams-text",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ),
    ] {
        let r = run_cassette(
            &format!("openai-compatible-chat/{name}"),
            Family::Compatible,
            model,
            true,
            BodyMatch::Bytes,
            text_turn("You are concise.", "Reply with exactly: Hello!", 20.0, t),
        )
        .await;
        assert_eq!(r[0].text().trim(), "Hello!", "{name}");
    }
    for (name, model) in [
        ("groq-streams-tool-call", "llama-3.3-70b-versatile"),
        ("openrouter-streams-tool-call", "openai/gpt-4o-mini"),
        (
            "togetherai-streams-tool-call",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ),
    ] {
        let r = run_cassette(
            &format!("openai-compatible-chat/{name}"),
            Family::Compatible,
            model,
            true,
            BodyMatch::Bytes,
            tool_call_turn(t),
        )
        .await;
        assert_eq!(r[0].tool_calls().count(), 1, "{name}");
    }
    for (name, model, prompt) in [
        (
            "groq-llama-3-3-70b-drives-a-tool-loop",
            "llama-3.3-70b-versatile",
            LOOP,
        ),
        (
            "openrouter-claude-opus-4-7-drives-a-tool-loop",
            "anthropic/claude-opus-4.7",
            LOOP_ONCE,
        ),
        (
            "openrouter-gpt-4o-mini-drives-a-tool-loop",
            "openai/gpt-4o-mini",
            LOOP_ONCE,
        ),
        (
            "openrouter-gpt-5-5-drives-a-tool-loop",
            "openai/gpt-5.5",
            LOOP_ONCE,
        ),
    ] {
        let r = run_cassette(
            &format!("openai-compatible-chat/{name}"),
            Family::Compatible,
            model,
            true,
            BodyMatch::Bytes,
            tool_loop(prompt, t),
        )
        .await;
        assert!(r[1].text().contains("Paris"), "{name}");
    }
}

/// The 2026-05-06 Responses recordings predate the `store: false` route default and the current
/// body encode order (`stream` last), so they replay on a route without defaults, JSON-equal.
#[tokio::test]
async fn openai_responses_cassettes() {
    let r = run_cassette(
        "openai-responses/gpt-5-5-streams-text",
        Family::Responses,
        "gpt-5.5",
        false,
        BodyMatch::Json,
        text_turn("You are concise.", "Reply with exactly: Hello!", 80.0, None),
    )
    .await;
    assert_eq!(r[0].text(), "Hello!");
    let r = run_cassette(
        "openai-responses/gpt-5-5-streams-tool-call",
        Family::Responses,
        "gpt-5.5",
        false,
        BodyMatch::Json,
        tool_call_turn(None),
    )
    .await;
    assert_eq!(
        r[0].finish_reason,
        opencode_llm_api::FinishReason::ToolCalls
    );
    let r = run_cassette(
        "openai-responses/gpt-5-5-drives-a-tool-loop",
        Family::Responses,
        "gpt-5.5",
        false,
        BodyMatch::Json,
        tool_loop(LOOP, None),
    )
    .await;
    assert!(r[1].text().contains("Paris"));
}

fn gpt5_facade_options(extra: Value) -> Option<opencode_llm_api::ProviderOptions> {
    // `withOpenAIOptions("gpt-5.5", ..., { textVerbosity: true })` defaults merged with `extra`.
    let mut base = json!({
        "store": false,
        "reasoningEffort": "medium",
        "reasoningSummary": "auto",
        "include": ["reasoning.encrypted_content"],
        "textVerbosity": "low",
    });
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    provider_options(base)
}

#[tokio::test]
async fn openai_responses_reasoning_cassettes() {
    let r = run_cassette(
        "openai-responses/openai-responses-gpt-5-5-reasoning",
        Family::Responses,
        "gpt-5.5",
        true,
        BodyMatch::Bytes,
        |_, mut request, _| {
            request.system = system(
                "Show concise reasoning when the provider supports visible reasoning summaries.",
            );
            request.messages = vec![user("Think briefly, then reply exactly with: Hello!")];
            request.generation = Some(generation(120.0, None));
            request.provider_options = gpt5_facade_options(
                json!({ "reasoningEffort": "low", "reasoningSummary": "auto" }),
            );
            request
        },
    )
    .await;
    assert!(
        r[0].usage
            .as_ref()
            .and_then(|u| u.reasoning_tokens)
            .unwrap_or(0.0)
            > 0.0
    );

    let encrypted = json!({
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "reasoningEffort": "low",
        "reasoningSummary": "auto",
    });
    run_cassette(
        "openai-responses/openai-responses-gpt-5-5-reasoning-continuation",
        Family::Responses,
        "gpt-5.5",
        true,
        BodyMatch::Bytes,
        move |turn, mut request, prev| {
            request.provider_options = gpt5_facade_options(encrypted.clone());
            if turn == 0 {
                request.system = system("Show concise reasoning when the provider supports visible reasoning summaries.");
                request.messages = vec![user("Think briefly, then reply exactly with: Hello!")];
                request.generation = Some(generation(120.0, None));
                return request;
            }
            // `assistantMessageFromResponse` for `openai-encrypted` reasoning.
            let (prev_request, response) = prev.unwrap();
            let end = response
                .events
                .iter()
                .find_map(|e| match e {
                    LlmEvent::ReasoningEnd { provider_metadata, .. }
                        if provider_metadata
                            .as_ref()
                            .and_then(|m| m.get("openai"))
                            .and_then(|m| m.get("itemId"))
                            .is_some_and(Value::is_string) =>
                    {
                        provider_metadata.clone()
                    }
                    _ => None,
                })
                .expect("reasoning metadata");
            let mut messages = prev_request.messages.clone();
            messages.push(Message::assistant(vec![
                ContentPart::Reasoning {
                    text: response.reasoning(),
                    encrypted: None,
                    metadata: None,
                    provider_metadata: Some(end),
                },
                ContentPart::text(response.text()),
            ]));
            messages.push(user("Now reply exactly with: Done."));
            request.messages = messages;
            request.generation = Some(generation(40.0, None));
            request
        },
    )
    .await;
}

#[tokio::test]
async fn openai_responses_image_tool_result_cassette() {
    let png = std::fs::read(format!(
        "{}/tests/fixtures/openai-media/restroom.png",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    use base64::Engine;
    let image = base64::engine::general_purpose::STANDARD.encode(png);
    let r = run_cassette(
        "openai-responses/openai-responses-gpt-5-5-image-tool-result",
        Family::Responses,
        "gpt-5.5",
        true,
        BodyMatch::Bytes,
        move |_, mut request, _| {
            request.system = system("Read images carefully. Reply only with the visible text, lowercase, no punctuation.");
            request.generation = Some(generation(40.0, None));
            request.provider_options = gpt5_facade_options(json!({}));
            request.tools = vec![ToolDefinition {
                name: "read_screenshot".into(),
                description: "Capture a screenshot of the current screen.".into(),
                input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false })
                    .as_object()
                    .unwrap()
                    .clone(),
                output_schema: None,
                cache: None,
                metadata: None,
                native: None,
            }];
            request.messages = vec![
                user("Use the read_screenshot tool, then reply with the words shown."),
                Message::assistant(vec![ContentPart::ToolCall(ToolCallPart {
                    id: "call_screenshot_1".into(),
                    name: "read_screenshot".into(),
                    input: json!({}),
                    provider_executed: None,
                    metadata: None,
                    provider_metadata: None,
                })]),
                Message::new(
                    MessageRole::Tool,
                    vec![ContentPart::ToolResult(ToolResultPart {
                        id: "call_screenshot_1".into(),
                        name: "read_screenshot".into(),
                        result: ToolResultValue::Content(vec![
                            ToolContent::Text { text: "Image read successfully".into() },
                            ToolContent::File {
                                uri: format!("data:image/png;base64,{image}"),
                                mime: "image/png".into(),
                                name: None,
                            },
                        ]),
                        provider_executed: None,
                        cache: None,
                        metadata: None,
                        provider_metadata: None,
                    })],
                ),
            ];
            request
        },
    )
    .await;
    assert_eq!(r[0].text(), "jiggling restroom prison");
}

/// Recorded before the current encode order (`store` last), so compared as JSON.
#[tokio::test]
async fn openai_responses_cache_cassette() {
    let large = "You are a concise, factual assistant. Answer precisely and avoid filler. Cite numbers when known. ".repeat(250);
    let r = run_cassette(
        "openai-responses-cache/reports-cached-tokens-on-identical-second-call",
        Family::Responses,
        "gpt-4.1-mini",
        true,
        BodyMatch::Json,
        move |_, mut request, _| {
            request.system = system(&large);
            request.messages = vec![user("Say hi.")];
            request.generation = Some(generation(16.0, Some(0.0)));
            request.provider_options = provider_options(
                json!({ "store": false, "promptCacheKey": "recorded-cache-test" }),
            );
            request
        },
    )
    .await;
    assert_eq!(
        r[0].usage.as_ref().unwrap().cache_read_input_tokens,
        Some(0.0)
    );
    assert_eq!(
        r[1].usage.as_ref().unwrap().cache_read_input_tokens,
        Some(4608.0)
    );
}
