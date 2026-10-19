//! Purpose: unit-level contract tests for provider transform and error rules ported from
//! packages/opencode/src/provider/{transform,error}.ts, with cases copied from
//! packages/opencode/test/provider/{transform,error}.test.ts.
//! Scope: pure functions; no I/O.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! |-- provider-adapters/src/providers/error.rs
//! `-- provider-adapters/src/providers/transform.rs
//! External:
//! |-- pretty_assertions::assert_eq
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- transform::temperature
//! |-- transform::top_k
//! `-- transform::top_p
//! Used by:
//! `-- none, entrypoint
//! Invariants: expectations come from the TS tests or TS source, not from Rust output.
//! Contract tests: provider-adapters/tests/providers_transform.rs

use opencode_llm_api::JsonObject;
use opencode_provider_adapters::providers::error::{
    ApiCallError, ParsedApiCallError, ParsedStreamError, api_error_message, parse_api_call_error,
    parse_stream_error,
};
use opencode_provider_adapters::providers::transform::{
    self, OUTPUT_TOKEN_MAX, TransformModel, max_output_tokens, provider_options, small_options,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

fn obj(v: Value) -> JsonObject {
    v.as_object().cloned().unwrap()
}

fn model(provider: &str, id: &str, npm: &str) -> TransformModel {
    TransformModel {
        provider_id: provider.into(),
        api_id: id.into(),
        npm: npm.into(),
        url: "https://api.test.com".into(),
        output_limit: 64_000,
        reasoning: true,
        input_modalities: vec![],
    }
}

#[test]
fn provider_options_sdk_keys_and_openai_reasoning_gate() {
    let bedrock = model(
        "my-bedrock",
        "anthropic.claude-sonnet-4",
        "@ai-sdk/amazon-bedrock",
    );
    assert_eq!(
        provider_options(
            &bedrock,
            &obj(json!({ "cachePoint": { "type": "default" } }))
        ),
        obj(json!({ "bedrock": { "cachePoint": { "type": "default" } } }))
    );
    let meta = TransformModel {
        input_modalities: vec![],
        reasoning: false,
        ..model("meta", "muse-spark", "@ai-sdk/openai")
    };
    assert_eq!(
        provider_options(
            &meta,
            &obj(json!({ "reasoningEffort": "xhigh", "reasoningSummary": "auto" }))
        ),
        obj(
            json!({ "openai": { "reasoningEffort": "xhigh", "reasoningSummary": "auto", "forceReasoning": true } })
        )
    );
    let openai = model("test", "test-model", "@ai-sdk/openai");
    assert_eq!(
        provider_options(&openai, &obj(json!({ "store": false }))),
        obj(json!({ "openai": { "store": false, "forceReasoning": true } }))
    );
    let xai = model("my-xai", "grok-4", "@ai-sdk/xai");
    assert_eq!(
        provider_options(&xai, &obj(json!({ "promptCacheKey": "session" }))),
        obj(json!({ "xai": { "promptCacheKey": "session" } }))
    );
    let cf = model(
        "cloudflare-ai-gateway",
        "openai/gpt-5.4",
        "ai-gateway-provider",
    );
    assert_eq!(
        provider_options(&cf, &obj(json!({ "reasoningEffort": "high" }))),
        obj(json!({ "openaiCompatible": { "reasoningEffort": "high" } }))
    );
    let merge = model(
        "merge-gateway",
        "openai/gpt-5.6-sol",
        "merge-gateway-ai-sdk-provider",
    );
    assert_eq!(
        provider_options(&merge, &obj(json!({ "reasoningEffort": "high" }))),
        obj(json!({ "mergeGateway": { "reasoningEffort": "high" } }))
    );
    let azure = model("azure", "gpt-4o", "@ai-sdk/azure");
    let out = provider_options(&azure, &obj(json!({ "store": false })));
    assert_eq!(out["openai"], out["azure"]);
    let dotted = model("wafer.ai", "m", "@ai-sdk/openai-compatible");
    assert!(provider_options(&dotted, &JsonObject::new()).contains_key("wafer"));
    let other = model("custom.x", "m", "@ai-sdk/some-other");
    assert!(provider_options(&other, &JsonObject::new()).contains_key("custom.x"));
}

#[test]
fn gateway_options_split_under_upstream_slug() {
    let m = model("vercel-gw", "amazon/nova-2-lite", "@ai-sdk/gateway");
    assert_eq!(
        provider_options(&m, &obj(json!({ "gateway": { "order": ["a"] }, "x": 1 }))),
        obj(json!({ "gateway": { "order": ["a"] }, "bedrock": { "x": 1 } }))
    );
    let bare = model("gw", "no-slug", "@ai-sdk/gateway");
    assert_eq!(
        provider_options(
            &bare,
            &obj(json!({ "gateway": { "order": ["a"] }, "x": 1 }))
        ),
        obj(json!({ "gateway": { "order": ["a"], "x": 1 } }))
    );
    assert_eq!(
        provider_options(&bare, &obj(json!({ "x": 1 }))),
        obj(json!({ "gateway": { "x": 1 } }))
    );
}

#[test]
fn anthropic_block_binding_matrix() {
    let binding = json!({ "prefixMismatchBehavior": "drop_block" });
    let sdks = [
        ("@ai-sdk/anthropic", "anthropic", "thinking"),
        ("@ai-sdk/google-vertex/anthropic", "anthropic", "thinking"),
        ("@ai-sdk/amazon-bedrock", "bedrock", "reasoningConfig"),
    ];
    let binds = [
        "claude-fable-5-1",
        "claude-fable-5.1",
        "claude-5.1-fable",
        "global.anthropic.claude-fable-5-1",
        "us.anthropic.claude-fable-5-1-v1:0",
        "claude-fable-5-1@default",
        "CLAUDE-FABLE-5-1",
        "claude-opus-5-1",
        "claude-sonnet-5-2",
        "claude-mythos-5-2",
        "claude-mythos-5-10",
        "claude-opus-6",
        "claude-6-opus",
        "claude-mythos-6-20270901",
    ];
    let leaves = [
        "claude-haiku-4-5",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-opus-5-0",
        "claude-opus-5-20260724",
        "global.anthropic.claude-opus-5",
        "us.anthropic.claude-opus-5",
        "claude-sonnet-5@default",
        "claude-mythos-5-1",
        "claude-mythos-5.1",
        "claude-5.1-mythos",
        "global.anthropic.claude-mythos-5-1-v1:0",
        "claude-mythos-5-1@default",
        "CLAUDE-MYTHOS-5-1",
        "claude-future",
    ];
    for (npm, key, option) in sdks {
        for id in binds {
            let m = model("custom", id, npm);
            assert_eq!(
                provider_options(&m, &JsonObject::new()),
                obj(json!({ key: { option: { "type": "adaptive", "blockBinding": binding } } })),
                "{npm} {id}"
            );
            assert_eq!(
                provider_options(
                    &m,
                    &obj(json!({ option: { "type": "enabled", "budgetTokens": 4000 } }))
                ),
                obj(
                    json!({ key: { option: { "type": "enabled", "budgetTokens": 4000, "blockBinding": binding } } })
                ),
                "{npm} {id}"
            );
            assert_eq!(
                provider_options(&m, &obj(json!({ option: { "type": "disabled" } }))),
                obj(json!({ key: { option: { "type": "disabled" } } })),
            );
        }
        for id in leaves {
            let m = model("custom", id, npm);
            assert_eq!(
                provider_options(&m, &JsonObject::new()),
                obj(json!({ key: {} })),
                "{npm} {id}"
            );
        }
        for id in ["claude-fable-5-1", "claude-opus-5"] {
            let m = model("custom", id, npm);
            assert_eq!(
                provider_options(
                    &m,
                    &obj(json!({ option: { "type": "adaptive", "blockBinding": false } }))
                ),
                obj(json!({ key: { option: { "type": "adaptive" } } }))
            );
            assert_eq!(
                provider_options(&m, &obj(json!({ option: { "blockBinding": false } }))),
                obj(json!({ key: {} }))
            );
            let custom = json!({ option: { "type": "adaptive", "blockBinding": { "prefixMismatchBehavior": "error" } } });
            assert_eq!(
                provider_options(&m, &obj(custom.clone())),
                obj(json!({ key: custom }))
            );
        }
    }
}

#[test]
fn sampling_defaults() {
    let t = |id: &str| transform::temperature(&model("p", id, "x"));
    assert_eq!(t("claude-sonnet-4"), None);
    assert_eq!(t("gemini-2.5-pro"), Some(1.0));
    assert_eq!(t("gemini-3.5-flash"), Some(1.0));
    assert_eq!(t("gemini-3.5-flash-lite"), None);
    assert_eq!(t("gemini-2.0-flash"), None);
    assert_eq!(t("kimi-k2-instruct"), Some(0.6));
    assert_eq!(t("kimi-k2.5"), Some(1.0));
    assert_eq!(t("glm-4.7"), Some(1.0));
    assert_eq!(t("cohere/north-mini-code"), Some(1.0));
    assert_eq!(t("gpt-4o"), None);
    let p = |provider: &str, id: &str| transform::top_p(&model(provider, id, "x"));
    assert_eq!(p("x", "gemini-3-pro-preview"), Some(0.95));
    assert_eq!(p("x", "minimax-m2"), Some(0.95));
    assert_eq!(p("deepseek", "deepseek-v4-flash"), Some(0.95));
    assert_eq!(p("opencode-go", "deepseek-v4-flash"), Some(0.95));
    assert_eq!(p("other", "deepseek-v4-flash"), None);
    assert_eq!(p("other", "deepseek-v4-flash-0731"), Some(0.95));
    let k = |id: &str| transform::top_k(&model("p", id, "x"));
    assert_eq!(k("minimax-m2"), Some(20.0));
    assert_eq!(k("minimax-m2.5"), Some(40.0));
    assert_eq!(k("gemini-3.1-pro"), Some(64.0));
    assert_eq!(k("gpt-5"), None);
    assert!(transform::is_legacy_gemini("gemini-1.5-pro"));
    assert!(transform::is_legacy_gemini("gemini-flash-2"));
    assert!(!transform::is_legacy_gemini("gemini-3-pro"));
}

#[test]
fn max_output_tokens_and_small_options() {
    let mut m = model("p", "x", "y");
    assert_eq!(max_output_tokens(&m, OUTPUT_TOKEN_MAX), 32_000);
    m.output_limit = 8_192;
    assert_eq!(max_output_tokens(&m, OUTPUT_TOKEN_MAX), 8_192);
    m.output_limit = 0;
    assert_eq!(max_output_tokens(&m, OUTPUT_TOKEN_MAX), 32_000);
    let openai = model("openai", "gpt-5", "@ai-sdk/openai");
    assert_eq!(
        small_options(&openai, Some(&obj(json!({ "reasoningEffort": "low" })))),
        obj(json!({ "store": false, "reasoningEffort": "low" }))
    );
    let or = model(
        "openrouter",
        "google/gemini-2.5-flash",
        "@openrouter/ai-sdk-provider",
    );
    assert_eq!(
        small_options(&or, None),
        obj(json!({ "reasoning": { "enabled": false } }))
    );
    let venice = model("venice", "v", "venice-ai-sdk-provider");
    assert_eq!(
        small_options(&venice, None),
        obj(json!({ "veniceParameters": { "disableThinking": true } }))
    );
    assert_eq!(
        transform::sanitize_surrogates(&[0x61, 0xD800, 0x62]),
        "a\u{FFFD}b"
    );
    assert_eq!(
        transform::sdk_key("@ai-sdk/github-copilot"),
        Some("copilot")
    );
    assert_eq!(transform::sdk_key("unknown"), None);
}

#[test]
fn stream_errors() {
    for message in [
        "The model is currently at capacity due to high demand.",
        "The model is temporarily unavailable.",
    ] {
        let input = json!({ "type": "error", "error": { "message": message } });
        assert_eq!(
            parse_stream_error(&input),
            Some(ParsedStreamError::ApiError {
                message: message.into(),
                is_retryable: true,
                response_body: input.to_string(),
            })
        );
    }
    let overflow = json!({ "type": "error", "error": { "code": "context_length_exceeded" } });
    assert!(matches!(
        parse_stream_error(&overflow),
        Some(ParsedStreamError::ContextOverflow { .. })
    ));
    let quota = json!({ "message": json!({ "type": "error", "error": { "code": "insufficient_quota" } }).to_string() });
    assert_eq!(
        parse_stream_error(&quota),
        Some(ParsedStreamError::ApiError {
            message: "Quota exceeded. Check your plan and billing details.".into(),
            is_retryable: false,
            response_body: json!({ "type": "error", "error": { "code": "insufficient_quota" } })
                .to_string(),
        })
    );
    assert_eq!(parse_stream_error(&json!({ "type": "message" })), None);
    assert_eq!(parse_stream_error(&json!("not json")), None);
}

#[test]
fn api_call_errors() {
    let e = |message: &str, status: Option<u16>, body: Option<&str>| ApiCallError {
        message: message.into(),
        status_code: status,
        response_body: body.map(str::to_owned),
        ..ApiCallError::default()
    };
    assert_eq!(
        api_error_message(&e("", Some(429), None)),
        "Too Many Requests"
    );
    assert_eq!(api_error_message(&e("", None, Some(" raw "))), "raw");
    assert_eq!(api_error_message(&e("", None, None)), "Unknown error");
    // `body.message || body.error || body.error?.message`: an `error` object is truthy and not a
    // string, so the baseline falls through to the raw body.
    assert_eq!(
        api_error_message(&e(
            "Bad Request",
            Some(400),
            Some(r#"{"error":{"message":"bad"}}"#)
        )),
        r#"Bad Request: {"error":{"message":"bad"}}"#
    );
    assert_eq!(
        api_error_message(&e("Bad Request", Some(400), Some(r#"{"error":"bad"}"#))),
        "Bad Request: bad"
    );
    assert_eq!(
        api_error_message(&e("Custom", Some(400), Some("body"))),
        "Custom"
    );
    assert!(
        api_error_message(&e("Unauthorized", Some(401), Some("<!DOCTYPE html><p>")))
            .starts_with("Unauthorized: request was blocked by a gateway or proxy.")
    );
    assert_eq!(
        api_error_message(&e("Bad Gateway", Some(502), Some("<html>x</html>"))),
        "Bad Gateway"
    );
    assert_eq!(
        api_error_message(&e("Bad Gateway", Some(502), Some("oops"))),
        "Bad Gateway: oops"
    );
    assert!(matches!(
        parse_api_call_error("anthropic", &e("x", Some(413), None)),
        ParsedApiCallError::ContextOverflow { .. }
    ));
    assert!(matches!(
        parse_api_call_error(
            "openai",
            &e(
                "x",
                Some(400),
                Some(r#"{"error":{"code":"context_length_exceeded"}}"#)
            )
        ),
        ParsedApiCallError::ContextOverflow { .. }
    ));
    let not_found = ApiCallError {
        url: Some("https://api.openai.com/v1/responses".into()),
        ..e("Not Found", Some(404), None)
    };
    match parse_api_call_error("openai", &not_found) {
        ParsedApiCallError::ApiError {
            is_retryable,
            metadata,
            ..
        } => {
            assert!(is_retryable);
            assert_eq!(
                metadata.unwrap()["url"],
                "https://api.openai.com/v1/responses"
            );
        }
        other => panic!("{other:?}"),
    }
    match parse_api_call_error("anthropic", &e("Not Found", Some(404), None)) {
        ParsedApiCallError::ApiError { is_retryable, .. } => assert!(!is_retryable),
        other => panic!("{other:?}"),
    }
}
