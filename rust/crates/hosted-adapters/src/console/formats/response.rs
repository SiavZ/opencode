//! Purpose: convert complete Zen provider responses through the baseline common response.
//! Scope: text, function calls, finish reasons, generated IDs and usage fields.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! Invariants: only first choices convert; malformed and already-common responses pass through.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use super::{Format, array, copy, generated, now, parsed, pick, stringify, text, truthy};
use serde_json::{Value, json};

pub(super) fn reason(value: &Value, from: Format, to: Format) -> Value {
    let common = match (from, text(value)) {
        (Format::Anthropic, "end_turn") => "stop",
        (Format::Anthropic, "tool_use") => "tool_calls",
        (Format::Anthropic, "max_tokens") => "length",
        (Format::Openai, "tool_call" | "tool_calls") => "tool_calls",
        (Format::Openai, "max_output_tokens" | "length") => "length",
        (_, "stop") if from != Format::Anthropic => "stop",
        (_, "tool_calls" | "length") if from == Format::OaCompat => text(value),
        (_, "content_filter") => "content_filter",
        _ => return Value::Null,
    };
    json!(match (to, common) {
        (Format::Anthropic, "stop") => "end_turn",
        (Format::Anthropic, "tool_calls") => "tool_use",
        (Format::Anthropic, "length") => "max_tokens",
        (Format::Openai, "tool_calls") => "tool_call",
        (Format::Openai, "length") => "max_output_tokens",
        _ => common,
    })
}

pub(super) fn common_usage(usage: &Value, from: Format, total_default: bool) -> Value {
    if from == Format::OaCompat {
        let mut out = pick(
            usage,
            &["prompt_tokens", "completion_tokens", "total_tokens"],
        );
        if truthy(&usage["prompt_tokens_details"]["cached_tokens"]) {
            out["prompt_tokens_details"] =
                json!({"cached_tokens":usage["prompt_tokens_details"]["cached_tokens"]});
        }
        return out;
    }
    let mut out = json!({});
    if usage["input_tokens"].is_number() {
        out["prompt_tokens"] = usage["input_tokens"].clone();
    }
    if usage["output_tokens"].is_number() {
        out["completion_tokens"] = usage["output_tokens"].clone();
    }
    if total_default || (usage["input_tokens"].is_number() && usage["output_tokens"].is_number()) {
        out["total_tokens"] = json!(
            usage["input_tokens"].as_i64().unwrap_or(0)
                + usage["output_tokens"].as_i64().unwrap_or(0)
        );
    }
    let cached = if from == Format::Anthropic {
        &usage["cache_read_input_tokens"]
    } else {
        &usage["input_tokens_details"]["cached_tokens"]
    };
    if if total_default {
        truthy(cached)
    } else {
        cached.is_number()
    } {
        out["prompt_tokens_details"] = json!({"cached_tokens":cached});
    }
    out
}

pub(super) fn output_usage(usage: &Value, to: Format) -> Value {
    if to == Format::OaCompat {
        return common_usage(usage, Format::OaCompat, false);
    }
    let mut out = json!({});
    copy(&mut out, "input_tokens", usage, "prompt_tokens");
    copy(&mut out, "output_tokens", usage, "completion_tokens");
    if to == Format::Openai {
        copy(&mut out, "total_tokens", usage, "total_tokens");
        if truthy(&usage["prompt_tokens_details"]["cached_tokens"]) {
            out["input_tokens_details"] =
                json!({"cached_tokens":usage["prompt_tokens_details"]["cached_tokens"]});
        }
    } else {
        copy(
            &mut out,
            "cache_read_input_tokens",
            &usage["prompt_tokens_details"],
            "cached_tokens",
        );
    }
    out
}

pub(super) fn from_response(body: Value, format: Format) -> Value {
    if format != Format::OaCompat && body["choices"].is_array() {
        return body;
    }
    if format == Format::Anthropic && body["type"] != "message" {
        return body;
    }
    let source = if format == Format::Openai {
        body.get("response")
            .filter(|value| !value.is_null())
            .unwrap_or(&body)
    } else {
        &body
    };
    if !source.is_object() {
        return body;
    }
    let choice = array(&body["choices"]).first();
    if format == Format::OaCompat && choice.is_none_or(|value| !truthy(&value["message"])) {
        return body;
    }
    let mut message = json!({"role":"assistant"});
    let mut calls = Vec::new();
    let mut output_text = String::new();
    if format == Format::OaCompat {
        let message_in = &choice.unwrap()["message"];
        if message_in["content"].is_string() {
            output_text.push_str(text(&message_in["content"]));
        }
        for tc in array(&message_in["tool_calls"])
            .iter()
            .filter(|tc| tc["type"] == "function" && truthy(&tc["function"]))
        {
            let mut call = pick(tc, &["id", "type"]);
            let mut function = pick(&tc["function"], &["name"]);
            if let Some(value) = tc["function"].get("arguments") {
                function["arguments"] = json!(stringify(&parsed(value)));
            }
            call["function"] = function;
            calls.push(call);
        }
    } else {
        if format == Format::Openai {
            output_text = array(&source["output"])
                .iter()
                .filter(|item| item["type"] == "message")
                .flat_map(|item| array(&item["content"]))
                .filter(|part| part["type"] == "output_text" && part["text"].is_string())
                .map(|part| text(&part["text"]))
                .collect();
        } else {
            output_text = array(&source["content"])
                .iter()
                .filter(|part| part["type"] == "text" && part["text"].is_string())
                .map(|part| text(&part["text"]))
                .collect();
        }
        let items = if format == Format::Openai {
            &source["output"]
        } else {
            &source["content"]
        };
        for item in array(items).iter().filter(|item| {
            item["type"]
                == if format == Format::Openai {
                    "function_call"
                } else {
                    "tool_use"
                }
        }) {
            let id = item["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| generated("toolu_"));
            let mut function = pick(item, &["name"]);
            function["arguments"] = json!(stringify(
                item.get(if format == Format::Openai {
                    "arguments"
                } else {
                    "input"
                })
                .filter(|value| !value.is_null())
                .unwrap_or(&json!({}))
            ));
            calls.push(json!({"id":id,"type":"function","function":function}));
        }
    }
    if !output_text.is_empty() {
        message["content"] = json!(output_text);
    }
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    let mut out = json!({"object":"chat.completion","created":now()});
    if format == Format::OaCompat {
        copy(&mut out, "id", &body, "id");
        copy(&mut out, "model", &body, "model");
    } else {
        out["id"] = json!(
            source["id"]
                .as_str()
                .map(|id| id.replacen(
                    if format == Format::Openai {
                        "resp_"
                    } else {
                        "msg_"
                    },
                    "chatcmpl_",
                    usize::from(id.starts_with(if format == Format::Openai {
                        "resp_"
                    } else {
                        "msg_"
                    }))
                ))
                .unwrap_or_else(|| generated("chatcmpl_"))
        );
        if let Some(model) = source
            .get("model")
            .filter(|value| !value.is_null())
            .or_else(|| body.get("model"))
        {
            out["model"] = model.clone();
        }
    }
    let finish = if format == Format::OaCompat {
        &choice.unwrap()["finish_reason"]
    } else {
        &source["stop_reason"]
    };
    out["choices"] = json!([{"index":0,"message":message,"finish_reason":reason(finish,format,Format::OaCompat)}]);
    if let Some(usage) = source
        .get("usage")
        .filter(|value| truthy(value))
        .or_else(|| body.get("usage").filter(|value| truthy(value)))
    {
        out["usage"] = common_usage(usage, format, false);
    }
    out
}

pub(super) fn to_response(body: Value, format: Format) -> Value {
    if format == Format::OaCompat {
        return body;
    }
    let Some(choice) = array(&body["choices"]).first() else {
        return body;
    };
    if !truthy(&choice["message"]) {
        return body;
    }
    let message = &choice["message"];
    let mut content = Vec::new();
    if message["content"].is_string() && truthy(&message["content"]) {
        content.push(if format == Format::Openai { json!({"id":generated("msg_"),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":message["content"],"annotations":[],"logprobs":[]}]}) } else { json!({"type":"text","text":message["content"]}) });
    }
    for tc in array(&message["tool_calls"])
        .iter()
        .filter(|tc| tc["type"] == "function" && truthy(&tc["function"]))
    {
        let mut item =
            json!({"type":if format == Format::Openai {"function_call"} else {"tool_use"}});
        copy(&mut item, "id", tc, "id");
        copy(&mut item, "name", &tc["function"], "name");
        if format == Format::Openai {
            copy(&mut item, "call_id", tc, "id");
            copy(&mut item, "arguments", &tc["function"], "arguments");
        } else if let Some(value) = tc["function"].get("arguments") {
            item["input"] = parsed(value);
        }
        content.push(item);
    }
    let mut out = pick(&body, &["model"]);
    if format == Format::Openai {
        out["id"] = json!(
            body["id"]
                .as_str()
                .map(|id| if let Some(rest) = id.strip_prefix("chatcmpl_") {
                    format!("resp_{rest}")
                } else {
                    id.to_owned()
                })
                .unwrap_or_else(|| generated("resp_"))
        );
        out["object"] = json!("response");
        out["output"] = json!(content);
    } else {
        copy(&mut out, "id", &body, "id");
        out["type"] = json!("message");
        out["role"] = json!("assistant");
        out["content"] = if content.is_empty() {
            json!([{"type":"text","text":""}])
        } else {
            json!(content)
        };
    }
    out["stop_reason"] = reason(&choice["finish_reason"], Format::OaCompat, format);
    if truthy(&body["usage"]) {
        out["usage"] = output_usage(&body["usage"], format);
    }
    out
}
