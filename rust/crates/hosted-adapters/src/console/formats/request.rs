//! Purpose: normalize Zen provider requests into the baseline common message representation.
//! Scope: Anthropic, Responses API and chat-completions input conversion.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! `-- hosted-adapters/src/console/formats/request_out.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! Invariants: preserves baseline role filtering, tool ordering and omitted optional fields.
//! Contract tests: hosted-adapters/tests/console_formats.rs

#[path = "request_out.rs"]
mod output;
use super::{Format, array, copy, image, pick, stringify, text, truthy};
pub(crate) use output::to_request;
use serde_json::{Value, json};

pub(super) fn from_request(body: Value, format: Format) -> Value {
    let mut messages = Vec::new();
    if format == Format::Anthropic {
        messages.extend(
            array(&body["system"])
                .iter()
                .filter(|part| {
                    text(&part["type"]) == "text"
                        && part["text"].is_string()
                        && truthy(&part["text"])
                })
                .map(|part| json!({"role":"system","content":part["text"]})),
        );
    }
    let input = if format == Format::Openai && body["input"].is_array() {
        &body["input"]
    } else {
        &body["messages"]
    };
    for message in array(input) {
        let role = text(&message["role"]);
        let content = &message["content"];
        if format == Format::Openai && role.is_empty() {
            match text(&message["type"]) {
                "function_call" => {
                    let mut tc = json!({"type":"function","function":{"arguments":stringify(message.get("arguments").filter(|value| !value.is_null()).unwrap_or(&json!({})))}});
                    copy(&mut tc, "id", message, "id");
                    copy(&mut tc["function"], "name", message, "name");
                    messages.push(json!({"role":"assistant","tool_calls":[tc]}));
                }
                "function_call_output" => {
                    let mut item = json!({"role":"tool"});
                    copy(&mut item, "tool_call_id", message, "call_id");
                    if let Some(value) = message.get("output") {
                        item["content"] = json!(stringify(value));
                    }
                    messages.push(item);
                }
                _ => (),
            }
            continue;
        }
        match role {
            "system" | "developer" if format != Format::Anthropic => {
                if role == "developer" && format != Format::Openai {
                    continue;
                }
                let value = if content.is_string() {
                    Some(content)
                } else if format == Format::Openai {
                    array(content)
                        .iter()
                        .find(|part| part["text"].is_string())
                        .map(|part| &part["text"])
                } else {
                    None
                };
                if let Some(value) = value.filter(|value| truthy(value)) {
                    messages.push(json!({"role":"system","content":value}));
                }
            }
            "user" => {
                if content.is_string() && format != Format::Anthropic {
                    messages.push(json!({"role":"user","content":content}));
                    continue;
                }
                let mut parts = Vec::new();
                for part in array(content) {
                    let kind = text(&part["type"]);
                    if (kind == "text" || (kind == "input_text" && format == Format::Openai))
                        && part["text"].is_string()
                    {
                        parts.push(json!({"type":"text","text":part["text"]}));
                    }
                    if format == Format::Anthropic
                        && kind == "image"
                        && let Some(value) = image(part)
                    {
                        parts.push(value);
                    }
                    if format == Format::Openai
                        && let Some(value) = image(part)
                    {
                        parts.push(value);
                    }
                    if format == Format::OaCompat && kind == "image_url" {
                        parts.push(pick(part, &["type", "image_url"]));
                    }
                    if kind == "tool_result" && matches!(format, Format::Anthropic | Format::Openai)
                    {
                        let mut item = json!({"role":"tool"});
                        copy(
                            &mut item,
                            "tool_call_id",
                            part,
                            if format == Format::Anthropic {
                                "tool_use_id"
                            } else {
                                "tool_call_id"
                            },
                        );
                        if let Some(value) = part.get("content") {
                            item["content"] = json!(stringify(value));
                        }
                        messages.push(item);
                    }
                }
                if parts.len() == 1 && parts[0]["type"] == "text" {
                    messages.push(json!({"role":"user","content":parts[0]["text"]}));
                } else if !parts.is_empty() {
                    messages.push(json!({"role":"user","content":parts}));
                }
            }
            "assistant" if format == Format::Anthropic => {
                let texts = array(content)
                    .iter()
                    .filter(|part| part["type"] == "text" && part["text"].is_string())
                    .map(|part| text(&part["text"]))
                    .collect::<String>();
                let tools = array(content).iter().filter(|part| part["type"] == "tool_use").map(|part| {
                    let mut tc = json!({"type":"function","function":{"arguments":stringify(part.get("input").filter(|value| !value.is_null()).unwrap_or(&json!({})))}});
                    copy(&mut tc,"id",part,"id"); copy(&mut tc["function"],"name",part,"name"); tc
                }).collect::<Vec<_>>();
                let mut item = json!({"role":"assistant","content":texts});
                if !tools.is_empty() {
                    item["tool_calls"] = json!(tools);
                }
                messages.push(item);
            }
            "assistant" => {
                let mut item = json!({"role":"assistant"});
                if content.is_string() && (format == Format::OaCompat || truthy(content)) {
                    item["content"] = content.clone();
                }
                if message["tool_calls"].is_array() {
                    item["tool_calls"] = message["tool_calls"].clone();
                }
                messages.push(item);
            }
            "tool" if format != Format::Anthropic => {
                let mut item = pick(message, &["tool_call_id", "content"]);
                item["role"] = json!("tool");
                messages.push(item);
            }
            _ => (),
        }
    }
    let mut out = pick(&body, &["model", "max_tokens", "temperature", "top_p"]);
    if format == Format::Openai
        && let Some(value) = body
            .get("max_output_tokens")
            .filter(|value| !value.is_null())
    {
        out["max_tokens"] = value.clone();
    }
    out["messages"] = json!(messages);
    out["stream"] = json!(truthy(&body["stream"]));
    if body["tools"].is_array() {
        out["tools"] = if format == Format::Anthropic {
            json!(
                array(&body["tools"])
                    .iter()
                    .filter(|tool| tool.get("input_schema").is_some())
                    .map(|tool| {
                        let mut f = pick(tool, &["name", "description"]);
                        copy(&mut f, "parameters", tool, "input_schema");
                        json!({"type":"function","function":f})
                    })
                    .collect::<Vec<_>>()
            )
        } else {
            body["tools"].clone()
        };
    }
    if format == Format::OaCompat {
        copy(&mut out, "tool_choice", &body, "tool_choice");
        copy(&mut out, "stop", &body, "stop");
        return out;
    }
    let tc = &body["tool_choice"];
    if format == Format::Anthropic {
        match text(&tc["type"]) {
            "auto" => out["tool_choice"] = json!("auto"),
            "any" => out["tool_choice"] = json!("required"),
            "tool" if tc["name"].is_string() => {
                out["tool_choice"] = json!({"type":"function","function":{"name":tc["name"]}})
            }
            _ => (),
        }
    } else if tc == "auto"
        || tc == "required"
        || (tc["type"] == "function" && truthy(&tc["function"]["name"]))
    {
        out["tool_choice"] = tc.clone();
    }
    let stop = body
        .get("stop_sequences")
        .filter(|value| !value.is_null())
        .or_else(|| {
            if format == Format::Openai {
                body.get("stop")
            } else {
                None
            }
        });
    if let Some(stop) = stop.filter(|value| truthy(value)) {
        if stop.is_string() {
            out["stop"] = stop.clone();
        }
        if let Some(values) = stop.as_array() {
            out["stop"] = if values.len() == 1 {
                values[0].clone()
            } else {
                stop.clone()
            };
        }
    }
    out
}
