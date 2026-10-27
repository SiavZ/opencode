//! Purpose: encode common requests into baseline provider-specific bodies.
//! Scope: Anthropic cache markers, Responses items/defaults and chat-completions output.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats/request.rs
//! Invariants: follows baseline defaults and deliberate provider-specific omission rules.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use super::super::{Format, array, copy, generated, image, parsed, pick, stringify, text, truthy};
use serde_json::{Value, json};

pub(crate) fn to_request(body: Value, format: Format) -> Value {
    let mut out = pick(
        &body,
        if format == Format::Anthropic {
            &["temperature", "top_p"][..]
        } else if format == Format::Openai {
            &["model", "top_p"][..]
        } else {
            &[
                "model",
                "max_tokens",
                "temperature",
                "top_p",
                "stop",
                "tool_choice",
                "response_format",
            ][..]
        },
    );
    let mut count = 0;
    let mut cached = |mut value: Value| {
        count += 1;
        if count <= 4 {
            value["cache_control"] = json!({"type":"ephemeral"});
        }
        value
    };
    let mut messages = Vec::new();
    if format == Format::Anthropic {
        let system = array(&body["messages"])
            .iter()
            .filter(|message| {
                message["role"] == "system"
                    && message["content"].is_string()
                    && truthy(&message["content"])
            })
            .map(|message| cached(json!({"type":"text","text":message["content"]})))
            .collect::<Vec<_>>();
        if !system.is_empty() {
            out["system"] = json!(system);
        }
        out["max_tokens"] = body
            .get("max_tokens")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(json!(32_000));
    }
    for message in array(&body["messages"]) {
        let content = &message["content"];
        match text(&message["role"]) {
            "system" if format != Format::Anthropic => {
                if content.is_string() && (format == Format::Openai || truthy(content)) {
                    messages.push(json!({"role":"system","content":content}));
                }
            }
            "user" => {
                if format == Format::OaCompat && content.is_string() {
                    messages.push(json!({"role":"user","content":content}));
                    continue;
                }
                let mut parts = Vec::new();
                if content.is_string() {
                    let part = json!({"type":if format == Format::Openai {"input_text"} else {"text"},"text":content});
                    parts.push(if format == Format::Anthropic {
                        cached(part)
                    } else {
                        part
                    });
                }
                for part in array(content) {
                    if part["type"] == "text" && part["text"].is_string() {
                        let value = json!({"type":if format == Format::Openai {"input_text"} else {"text"},"text":part["text"]});
                        parts.push(if format == Format::Anthropic {
                            cached(value)
                        } else {
                            value
                        });
                    }
                    if format != Format::Anthropic {
                        if let Some(mut image) = image(part) {
                            if format == Format::Openai {
                                image["type"] = json!("input_image");
                            }
                            parts.push(image);
                        }
                        continue;
                    }
                    if part["type"] != "image_url" {
                        continue;
                    }
                    let url = part["image_url"].get("url").unwrap_or(&part["image_url"]);
                    let Some(url) = url.as_str() else {
                        continue;
                    };
                    let source = url
                        .strip_prefix("data:")
                        .and_then(|rest| rest.split_once(";base64,"))
                        .filter(|(media, _)| !media.is_empty())
                        .map(
                            |(media, data)| json!({"type":"base64","media_type":media,"data":data}),
                        )
                        .unwrap_or_else(|| json!({"type":"url","url":url}));
                    parts.push(cached(json!({"type":"image","source":source})));
                }
                if parts.len() == 1 && parts[0]["type"] == "text" && format == Format::OaCompat {
                    messages.push(json!({"role":"user","content":parts[0]["text"]}));
                } else if !parts.is_empty() {
                    messages.push(json!({"role":"user","content":parts}));
                }
            }
            "assistant" if format == Format::OaCompat => {
                let mut item = json!({"role":"assistant"});
                if content.is_string() {
                    item["content"] = content.clone();
                }
                if message["tool_calls"].is_array() {
                    item["tool_calls"] = message["tool_calls"].clone();
                }
                messages.push(item);
            }
            "assistant" => {
                let mut parts = Vec::new();
                if content.is_string() && truthy(content) {
                    if format == Format::Openai {
                        messages.push(json!({"role":"assistant","content":[{"type":"output_text","text":content}]}));
                    } else {
                        parts.push(cached(json!({"type":"text","text":content})));
                    }
                }
                for tc in array(&message["tool_calls"])
                    .iter()
                    .filter(|tc| tc["type"] == "function" && truthy(&tc["function"]))
                {
                    if format == Format::Openai {
                        let mut item = json!({"type":"function_call"});
                        copy(&mut item, "call_id", tc, "id");
                        copy(&mut item, "name", &tc["function"], "name");
                        if let Some(value) = tc["function"].get("arguments") {
                            item["arguments"] = json!(stringify(value));
                        }
                        messages.push(item);
                        continue;
                    }
                    let mut item = json!({"type":"tool_use","id":if truthy(&tc["id"]) { tc["id"].clone() } else { json!(generated("toolu_")) }});
                    copy(&mut item, "name", &tc["function"], "name");
                    if let Some(value) = tc["function"].get("arguments") {
                        item["input"] = parsed(value);
                    }
                    parts.push(cached(item));
                }
                if !parts.is_empty() {
                    messages.push(json!({"role":"assistant","content":parts}));
                }
            }
            "tool" => {
                if format == Format::Anthropic {
                    let mut part = json!({"type":"tool_result"});
                    copy(&mut part, "tool_use_id", message, "tool_call_id");
                    copy(&mut part, "content", message, "content");
                    messages.push(json!({"role":"user","content":[cached(part)]}));
                } else if format == Format::Openai {
                    let mut item = json!({"type":"function_call_output"});
                    copy(&mut item, "call_id", message, "tool_call_id");
                    if let Some(value) = message.get("content") {
                        item["output"] = json!(stringify(value));
                    }
                    messages.push(item);
                } else {
                    let mut item = pick(message, &["tool_call_id", "content"]);
                    item["role"] = json!("tool");
                    messages.push(item);
                }
            }
            _ => (),
        }
    }
    out[if format == Format::Openai {
        "input"
    } else {
        "messages"
    }] = json!(messages);
    out["stream"] = json!(truthy(&body["stream"]));
    if body["tools"].is_array() {
        out["tools"] = json!(array(&body["tools"]).iter().filter(|tool| format != Format::Anthropic || tool["type"] == "function").map(|tool| {
            if format == Format::Anthropic { let mut item = pick(&tool["function"],&["name","description"]); copy(&mut item,"input_schema",&tool["function"],"parameters"); return cached(item); }
            if format == Format::Openai && tool["type"] == "function" { let mut item = pick(&tool["function"],&["name","description","parameters","strict"]); item["type"] = json!("function"); return item; }
            if format == Format::Openai { return tool.clone(); }
            // The baseline chat encoder reads flat tool fields, even when its common input has function nesting.
            json!({"type":"function","function":pick(tool,&["name","description","parameters"])})
        }).collect::<Vec<_>>());
    }
    if format == Format::OaCompat {
        return out;
    }
    if let Some(stop) = body.get("stop").filter(|value| truthy(value)) {
        if stop.is_array() {
            out["stop_sequences"] = stop.clone();
        }
        if stop.is_string() {
            out["stop_sequences"] = json!([stop]);
        }
    }
    let tc = &body["tool_choice"];
    if tc == "auto" || tc == "required" {
        out["tool_choice"] = if format == Format::Anthropic {
            json!({"type":if tc == "auto" {"auto"} else {"any"}})
        } else {
            tc.clone()
        };
    } else if tc["type"] == "function" && truthy(&tc["function"]["name"]) {
        out["tool_choice"] = if format == Format::Anthropic {
            json!({"type":"tool","name":tc["function"]["name"]})
        } else {
            tc.clone()
        };
    }
    if format == Format::Openai {
        copy(&mut out, "max_output_tokens", &body, "max_tokens");
        for key in ["truncation", "metadata", "store", "user"] {
            copy(&mut out, key, &body, key);
        }
        if body["include"].is_array() {
            out["include"] = body["include"].clone();
        }
        out["text"] =
            json!({"verbosity":if body["model"] == "gpt-5-codex" { "medium" } else { "low" }});
        out["reasoning"] = json!({"effort":"medium"});
    }
    out
}
