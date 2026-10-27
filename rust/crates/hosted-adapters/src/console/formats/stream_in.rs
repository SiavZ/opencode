//! Purpose: decode baseline SSE parts into common completion chunks.
//! Scope: Anthropic content blocks, OpenAI Responses events and chat-completions deltas.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/formats/response.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats/stream.rs
//! Invariants: malformed or unmatched frames pass through unchanged, matching baseline.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use super::super::response::{common_usage, reason};
use super::super::{Format, array, copy, now, pick, text, truthy};
use serde_json::{Value, json};

pub(super) fn from_part(part: &str, format: Format) -> Option<Value> {
    let lines = part.split('\n').collect::<Vec<_>>();
    let data = match format {
        Format::OaCompat => part.strip_prefix("data: "),
        Format::Anthropic => lines.iter().find_map(|line| line.strip_prefix("data: ")),
        Format::Openai => lines.get(1).and_then(|line| line.strip_prefix("data: ")),
        _ => return None,
    }?;
    let value = serde_json::from_str::<Value>(data).ok()?;
    let response = &value["response"];
    let mut out = json!({"id":value.get("id").filter(|value| !value.is_null()).cloned().unwrap_or(json!("")),"object":"chat.completion.chunk","created":now(),"model":value.get("model").filter(|value| !value.is_null()).cloned().unwrap_or(json!(""))});
    let mut choices = Vec::new();
    if format == Format::OaCompat {
        let choice = array(&value["choices"]).first()?;
        let delta = &choice["delta"];
        if !truthy(delta) {
            return None;
        }
        if let Some(created) = value.get("created").filter(|value| !value.is_null()) {
            out["created"] = created.clone();
        }
        let index = choice
            .get("index")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(json!(0));
        if truthy(&delta["content"]) {
            choices.push(
                json!({"index":index,"delta":{"content":delta["content"]},"finish_reason":null}),
            );
        }
        for tc in array(&delta["tool_calls"]) {
            let mut tc_out = pick(tc, &["id", "function"]);
            tc_out["index"] = tc
                .get("index")
                .filter(|value| !value.is_null())
                .cloned()
                .unwrap_or(json!(0));
            tc_out["type"] = tc
                .get("type")
                .filter(|value| !value.is_null())
                .cloned()
                .unwrap_or(json!("function"));
            choices
                .push(json!({"index":index,"delta":{"tool_calls":[tc_out]},"finish_reason":null}));
        }
        if truthy(&choice["finish_reason"]) {
            choices.push(json!({"index":index,"delta":{},"finish_reason":choice["finish_reason"]}));
        }
        if truthy(&value["usage"]) {
            out["usage"] = common_usage(&value["usage"], format, true);
        }
    } else if format == Format::Anthropic {
        if let Some(id) = value
            .get("id")
            .filter(|value| !value.is_null())
            .or_else(|| value.pointer("/message/id"))
        {
            out["id"] = id.clone();
        }
        if let Some(model) = value
            .get("model")
            .filter(|value| !value.is_null())
            .or_else(|| value.pointer("/message/model"))
        {
            out["model"] = model.clone();
        }
        let index = value
            .get("index")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(json!(0));
        let block = &value["content_block"];
        let delta = &value["delta"];
        match (text(&value["type"]),text(&block["type"]),text(&delta["type"])) {
            ("content_block_start","text",_) => choices.push(json!({"index":index,"delta":{"role":"assistant","content":""},"finish_reason":null})),
            ("content_block_start","tool_use",_) => {
                let mut tc = json!({"index":index,"type":"function","function":{"arguments":""}});
                copy(&mut tc,"id",block,"id"); copy(&mut tc["function"],"name",block,"name");
                choices.push(json!({"index":index,"delta":{"tool_calls":[tc]},"finish_reason":null}));
            }
            ("content_block_delta",_,"text_delta") => { let mut d = json!({}); copy(&mut d,"content",delta,"text"); choices.push(json!({"index":index,"delta":d,"finish_reason":null})); }
            ("content_block_delta",_,"input_json_delta") => {
                let mut function = json!({}); copy(&mut function,"arguments",delta,"partial_json");
                choices.push(json!({"index":index,"delta":{"tool_calls":[{"index":index,"function":function}]},"finish_reason":null}));
            }
            ("message_delta",_,_) => choices.push(json!({"index":0,"delta":{},"finish_reason":reason(&delta["stop_reason"],format,Format::OaCompat)})),
            _ => (),
        }
        if truthy(&value["usage"]) {
            out["usage"] = common_usage(&value["usage"], format, true);
        }
    } else {
        if let Some(id) = response
            .get("id")
            .filter(|value| !value.is_null())
            .or_else(|| value.get("id"))
        {
            out["id"] = id.clone();
        }
        if let Some(model) = response
            .get("model")
            .filter(|value| !value.is_null())
            .or_else(|| value.get("model"))
        {
            out["model"] = model.clone();
        }
        match lines.first()?.trim_start_matches("event: ").trim() {
            "response.output_text.delta" => {
                let delta = value
                    .get("delta")
                    .filter(|value| !value.is_null())
                    .or_else(|| value.get("text"))
                    .or_else(|| value.get("output_text_delta"));
                if let Some(delta) = delta.filter(|value| value.is_string() && truthy(value)) {
                    choices.push(json!({"index":0,"delta":{"content":delta},"finish_reason":null}));
                }
            }
            "response.output_item.added"
                if value["item"]["type"] == "function_call"
                    && value["item"]["name"].is_string()
                    && truthy(&value["item"]["name"]) =>
            {
                let mut tc = json!({"index":0,"type":"function","function":{"name":value["item"]["name"],"arguments":""}});
                copy(&mut tc, "id", &value["item"], "id");
                choices.push(json!({"index":0,"delta":{"tool_calls":[tc]},"finish_reason":null}));
            }
            "response.function_call_arguments.delta" => {
                let delta = value
                    .get("delta")
                    .filter(|value| !value.is_null())
                    .or_else(|| value.get("arguments_delta"));
                if let Some(delta) = delta.filter(|value| value.is_string() && truthy(value)) {
                    choices.push(json!({"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":delta}}]},"finish_reason":null}));
                }
            }
            "response.completed" => {
                let stop = response
                    .get("stop_reason")
                    .filter(|value| !value.is_null())
                    .unwrap_or(&value["stop_reason"]);
                choices.push(json!({"index":0,"delta":{},"finish_reason":reason(stop,format,Format::OaCompat)}));
                if truthy(&response["usage"]) {
                    out["usage"] = common_usage(&response["usage"], format, true);
                }
            }
            _ => (),
        }
    }
    out["choices"] = json!(choices);
    Some(out)
}
