//! Purpose: verify public Zen provider adapters against captured TypeScript baseline outputs.
//! Scope: offline fixture conversion, endpoint/body/header rules, usage and fragmented streams.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! |-- base64::{Engine, engine::general_purpose::STANDARD}
//! |-- http::HeaderMap
//! `-- serde_json::{Value, json}
//! Used by:
//! `-- none, entrypoint
//! Invariants: no live provider calls; expected conversions come from actual baseline functions.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use base64::{Engine, engine::general_purpose::STANDARD};
use opencode_hosted_adapters::console::formats::{
    BedrockDecoder, Format, FormatError, ProviderHelper, StreamConverter, UsageParser,
    build_cost_chunk, convert_request, convert_response, convert_stream_part,
};
use serde_json::{Value, json};

fn normalized(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("created") {
            object.insert("created".into(), json!(0));
        }
        if object.get("type").is_some_and(|value| value == "message")
            && object
                .get("status")
                .is_some_and(|value| value == "completed")
        {
            object.insert("id".into(), json!("<generated>"));
        }
        for item in object.values_mut() {
            *item = normalized(item.take());
        }
    }
    if let Some(array) = value.as_array_mut() {
        for item in array {
            *item = normalized(item.take());
        }
    }
    value
}
fn part_value(part: &str) -> Value {
    if let Some(data) = part.lines().find_map(|line| line.strip_prefix("data: "))
        && let Ok(value) = serde_json::from_str::<Value>(data)
    {
        return json!({"event":part.lines().find(|line| line.starts_with("event: ")),"data":normalized(value)});
    }
    serde_json::from_str::<Value>(part)
        .map(normalized)
        .unwrap_or_else(|_| json!(part))
}

#[test]
fn captured_typescript_baseline_matrix() {
    // Captured 2026-10-27 from packages/console/app/src/routes/zen/util/provider/*.ts,
    // executing actual createBody/Response/StreamPartConverter functions with isolated HOME/XDG.
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("console_formats_fixtures.json")).unwrap();
    assert_eq!(fixtures.len(), 120);
    for (index, fixture) in fixtures.iter().enumerate() {
        let from: Format = serde_json::from_value(fixture["from"].clone()).unwrap();
        let to: Format = serde_json::from_value(fixture["to"].clone()).unwrap();
        let input = fixture["input"].clone();
        let actual = match fixture["kind"].as_str().unwrap() {
            "request" => convert_request(input, from, to),
            "response" => convert_response(input, from, to),
            "stream" => convert_stream_part(input.as_str().unwrap(), from, to)
                .map(|value| part_value(&value)),
            _ => unreachable!(),
        };
        if fixture.get("error").is_some() {
            assert!(
                actual.is_err(),
                "fixture {index} baseline throws: {fixture}"
            );
            continue;
        }
        let expected = if fixture["kind"] == "stream" {
            part_value(fixture["expected"].as_str().unwrap())
        } else {
            normalized(fixture["expected"].clone())
        };
        assert_eq!(
            normalized(actual.unwrap()),
            expected,
            "fixture {index}: {fixture}"
        );
    }
}

#[test]
fn captured_typescript_helper_matrix() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("console_formats_helpers.json")).unwrap();
    assert_eq!(fixtures.len(), 11);
    for (index, fixture) in fixtures.iter().enumerate() {
        let format: Format = serde_json::from_value(fixture["format"].clone()).unwrap();
        let helper = ProviderHelper::new(
            format,
            fixture["reqModel"].as_str().unwrap(),
            fixture["providerModel"].as_str().unwrap(),
            fixture["adjustCacheUsage"].as_bool().unwrap_or(false),
        );
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "anthropic-version",
            http::HeaderValue::from_static("custom"),
        );
        headers.insert("anthropic-beta", http::HeaderValue::from_static("custom"));
        helper
            .modify_headers(&mut headers, "secret", "sticky")
            .unwrap();
        let headers = headers
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), json!(value.to_str().unwrap())))
            .collect::<serde_json::Map<_, _>>();
        let mut parser = UsageParser::new(format);
        for part in fixture["parts"].as_array().unwrap() {
            parser.parse(part.as_str().unwrap());
        }
        let mut actual = json!({"url":helper.modify_url("https://local.test",false),"streamUrl":helper.modify_url("https://local.test",true),"headers":headers,"body":helper.modify_body(json!({"model":"model","stream":true,"stream_options":{"old":true},"anthropic_beta":["old"],"messages":[]})),"usage":helper.normalize_usage(&fixture["usage"])});
        if let Some(parsed) = parser.retrieve() {
            actual["parsed"] = parsed.clone();
        }
        assert_eq!(actual, fixture["expected"], "helper fixture {index}");
    }
}

#[test]
fn native_helpers_cover_all_endpoints_headers_and_body_overrides() {
    for (format, path, key) in [
        (Format::Openai, "/responses", "authorization"),
        (Format::OaCompat, "/chat/completions", "authorization"),
        (Format::Anthropic, "/messages", "x-api-key"),
        (
            Format::Google,
            "/models/provider:generateContent",
            "x-goog-api-key",
        ),
        (Format::Systemone, "/systemone", "authorization"),
    ] {
        let helper = ProviderHelper::new(format, "claude-sonnet", "provider", false);
        assert_eq!(
            helper.modify_url("https://local.test", false),
            format!("https://local.test{path}")
        );
        let mut headers = http::HeaderMap::new();
        helper
            .modify_headers(&mut headers, "secret", "sticky")
            .unwrap();
        assert!(headers.contains_key(key));
        if format == Format::Anthropic {
            assert_eq!(headers["anthropic-beta"], "context-1m-2025-08-07");
            assert_eq!(headers["anthropic-version"], "2023-06-01");
        }
        if matches!(format, Format::OaCompat | Format::Systemone) {
            assert_eq!(headers["x-session-affinity"], "sticky");
        }
    }
    let chat = ProviderHelper::new(Format::OaCompat, "m", "m", false);
    assert_eq!(
        chat.modify_body(json!({"stream":true,"stream_options":{"other":true}}))["stream_options"],
        json!({"include_usage":true})
    );
    let bedrock = ProviderHelper::new(
        Format::Anthropic,
        "opus-4-6",
        "arn:aws:bedrock:region:account:model/a",
        false,
    );
    assert!(bedrock.is_bedrock());
    assert!(bedrock.modify_url("https://local.test", true).contains(
        "arn%3Aaws%3Abedrock%3Aregion%3Aaccount%3Amodel%2Fa/invoke-with-response-stream"
    ));
    let body = bedrock.modify_body(json!({"model":"m","stream":true,"messages":[]}));
    assert!(body.get("model").is_none() && body.get("stream").is_none());
    assert_eq!(body["anthropic_beta"], json!(["context-1m-2025-08-07"]));
    let databricks = ProviderHelper::new(Format::Anthropic, "haiku", "databricks-claude-x", false);
    assert!(!databricks.is_bedrock());
    assert_eq!(
        databricks.modify_body(json!({"model":"m","stream":true}))["model"],
        "m"
    );
    let mut headers = http::HeaderMap::new();
    databricks
        .modify_headers(&mut headers, "key", "id")
        .unwrap();
    assert_eq!(headers["authorization"], "Bearer key");
    let google = ProviderHelper::new(Format::Google, "g", "gemini", false);
    assert!(
        google
            .modify_url("https://local.test", true)
            .ends_with("gemini:streamGenerateContent?alt=sse")
    );
}

#[test]
fn billing_usage_keeps_provider_specific_cache_and_reasoning_semantics() {
    let helper = |format, adjust| ProviderHelper::new(format, "m", "m", adjust);
    let usage = helper(Format::Openai,false).normalize_usage(&json!({"input_tokens":100,"output_tokens":4,"input_tokens_details":{"cached_tokens":60,"cache_write_tokens":50},"output_tokens_details":{"reasoning_tokens":2}}));
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.cache_write5m_tokens, Some(50));
    assert_eq!(usage.reasoning_tokens, Some(2));
    let usage = helper(Format::OaCompat,true).normalize_usage(&json!({"prompt_tokens":11,"completion_tokens":3,"cached_tokens":0,"prompt_tokens_details":{"cache_creation_input_tokens":2}}));
    assert_eq!(usage.input_tokens, 2);
    assert_eq!(usage.cache_read_tokens, Some(9));
    assert_eq!(usage.cache_write5m_tokens, Some(2));
    let usage = helper(Format::Anthropic,false).normalize_usage(&json!({"input_tokens":3,"cache_creation_input_tokens":9,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":8}}));
    assert_eq!(usage.cache_write5m_tokens, Some(0));
    assert_eq!(usage.cache_write1h_tokens, Some(8));
    let usage = helper(Format::Google,false).normalize_usage(&json!({"promptTokenCount":10,"cachedContentTokenCount":4,"candidatesTokenCount":3,"thoughtsTokenCount":2}));
    assert_eq!(usage.input_tokens, 6);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(
        helper(Format::Systemone, false)
            .normalize_usage(&json!({}))
            .input_tokens,
        0
    );
    assert_eq!(
        helper(Format::Google, false).extract_usage(&json!({"usageMetadata":{"a":1}})),
        Some(json!({"a":1}))
    );
    assert_eq!(
        helper(Format::Openai, false).extract_usage(&json!({"response":{"usage":{"a":2}}})),
        Some(json!({"a":2}))
    );
}

#[test]
fn streaming_usage_merges_anthropic_nested_updates_and_ignores_noise() {
    let mut parser = UsageParser::new(Format::Anthropic);
    parser.parse("event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":4,\"cache_creation\":{\"ephemeral_5m_input_tokens\":5},\"server_tool_use\":{\"web_search_requests\":1}}}}");
    parser.parse("event: message_delta\ndata:{\"usage\":{\"output_tokens\":8,\"cache_creation\":{\"ephemeral_1h_input_tokens\":3}}}");
    parser.parse("event: ping\ndata: malformed");
    assert_eq!(
        parser.retrieve().unwrap(),
        &json!({"input_tokens":4,"output_tokens":8,"cache_creation":{"ephemeral_5m_input_tokens":5,"ephemeral_1h_input_tokens":3},"server_tool_use":{"web_search_requests":1}})
    );
    for (format, part) in [
        (
            Format::Openai,
            "event: response.completed\ndata: {\"response\":{\"usage\":{\"input_tokens\":2}}}",
        ),
        (Format::OaCompat, "data: {\"usage\":{\"prompt_tokens\":2}}"),
        (
            Format::Google,
            "data: {\"usageMetadata\":{\"promptTokenCount\":2}} ",
        ),
    ] {
        let mut parser = UsageParser::new(format);
        parser.parse(part);
        assert!(parser.retrieve().is_some());
        parser.parse("data: [DONE]");
        assert!(parser.retrieve().is_some());
    }
    let mut parser = UsageParser::new(Format::Systemone);
    parser.parse("data: {\"usage\":{}}");
    assert!(parser.retrieve().is_none());
}

#[test]
fn fragmented_utf8_sse_and_opaque_payloads_are_preserved() {
    let part = "data: {\"id\":\"a\",\"choices\":[{\"delta\":{\"content\":\"héllo\"}}]}";
    let expected = convert_stream_part(part, Format::OaCompat, Format::Openai).unwrap();
    for split in 0..=part.len() {
        let mut stream = StreamConverter::new(Format::OaCompat, Format::Openai);
        let mut out = stream.push(&part.as_bytes()[..split]).unwrap();
        out.extend(stream.push(&part.as_bytes()[split..]).unwrap());
        out.extend(stream.push(b"\r\n\r\n").unwrap());
        out.extend(stream.finish().unwrap());
        assert_eq!(String::from_utf8(out).unwrap(), format!("{expected}\n\n"));
    }
    let mut opaque = StreamConverter::new(Format::Systemone, Format::Systemone);
    assert_eq!(opaque.push(&[0, 255, 3]).unwrap(), vec![0, 255, 3]);
    assert!(matches!(
        convert_request(json!({}), Format::Google, Format::Openai),
        Err(FormatError::Unsupported { .. })
    ));
    assert_eq!(
        convert_stream_part("data: [DONE]", Format::OaCompat, Format::Openai).unwrap(),
        "data: [DONE]"
    );
    for format in [
        Format::Openai,
        Format::Anthropic,
        Format::Google,
        Format::Systemone,
        Format::OaCompat,
    ] {
        let chunk = build_cost_chunk(format, "0.03");
        assert!(chunk.ends_with("\n\n"));
        assert_eq!(part_value(&chunk)["data"]["cost"], "0.03");
    }
}

#[test]
fn finish_reasons_and_transport_terminal_edges() {
    for (chat, anthropic, responses) in [
        ("stop", "end_turn", "stop"),
        ("tool_calls", "tool_use", "tool_call"),
        ("length", "max_tokens", "max_output_tokens"),
        ("content_filter", "content_filter", "content_filter"),
    ] {
        let input = json!({"id":"chatcmpl_x","choices":[{"message":{"role":"assistant","content":""},"finish_reason":chat}]});
        let anth = convert_response(input.clone(), Format::OaCompat, Format::Anthropic).unwrap();
        assert_eq!(anth["stop_reason"], anthropic);
        assert_eq!(anth["content"], json!([{"type":"text","text":""}]));
        assert_eq!(
            convert_response(anth, Format::Anthropic, Format::OaCompat).unwrap()["choices"][0]["finish_reason"],
            chat
        );
        assert_eq!(
            convert_response(input, Format::OaCompat, Format::Openai).unwrap()["stop_reason"],
            responses
        );
    }
    for format in [
        Format::Openai,
        Format::Anthropic,
        Format::OaCompat,
        Format::Google,
        Format::Systemone,
    ] {
        assert_eq!(
            convert_request(Value::Null, format, format).unwrap(),
            Value::Null
        );
        assert_eq!(
            convert_response(json!({"unknown":{"a":1}}), format, format).unwrap(),
            json!({"unknown":{"a":1}})
        );
    }
    let mut stream = StreamConverter::new(Format::OaCompat, Format::Openai);
    assert!(stream.push(b"data: [DONE]").unwrap().is_empty());
    assert_eq!(stream.finish().unwrap(), b"data: [DONE]\n\n");
    assert!(stream.finish().unwrap().is_empty());
    let mut invalid = StreamConverter::new(Format::OaCompat, Format::Openai);
    invalid.push(&[255]).unwrap();
    assert!(invalid.finish().is_err());
}

fn aws_frame(message_type: &str, payload: Value) -> Vec<u8> {
    let name = b":message-type";
    let mut headers = vec![name.len() as u8];
    headers.extend(name);
    headers.push(7);
    headers.extend((message_type.len() as u16).to_be_bytes());
    headers.extend(message_type.as_bytes());
    let body = json!({"bytes":STANDARD.encode(payload.to_string()),"p":"padding"})
        .to_string()
        .into_bytes();
    let mut frame = Vec::new();
    frame.extend(((16 + headers.len() + body.len()) as u32).to_be_bytes());
    frame.extend((headers.len() as u32).to_be_bytes());
    let crc = |bytes: &[u8]| {
        !bytes.iter().fold(u32::MAX, |crc, byte| {
            (0..8).fold(crc ^ u32::from(*byte), |crc, _| {
                (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 }
            })
        })
    };
    frame.extend(crc(&frame).to_be_bytes());
    frame.extend(headers);
    frame.extend(body);
    frame.extend(crc(&frame).to_be_bytes());
    frame
}
#[test]
fn bedrock_binary_frames_handle_fragmentation_crc_and_truncation() {
    let payload =
        json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"héllo"}});
    let frame = aws_frame("event", payload.clone());
    for split in 0..=frame.len() {
        let mut decoder = BedrockDecoder::default();
        let mut output = decoder.push(&frame[..split]).unwrap();
        output.extend(decoder.push(&frame[split..]).unwrap());
        decoder.finish().unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("event: content_block_delta\ndata: {payload}\n\n")
        );
    }
    let mut corrupt = frame.clone();
    corrupt[12] ^= 1;
    assert!(BedrockDecoder::default().push(&corrupt).is_err());
    let mut decoder = BedrockDecoder::default();
    decoder.push(&frame[..7]).unwrap();
    assert!(decoder.finish().is_err());
    assert!(BedrockDecoder::default().push(&[0, 0, 0, 0]).is_err());
    assert!(
        BedrockDecoder::default()
            .push(&aws_frame("exception", payload))
            .unwrap()
            .is_empty()
    );
}
