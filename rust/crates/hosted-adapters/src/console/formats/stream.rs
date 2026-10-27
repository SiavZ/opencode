//! Purpose: translate baseline stream parts and frame fragmented SSE byte transport.
//! Scope: per-part wire conversion, cost notifications, bounded incremental transport buffers.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/formats.rs
//! |-- hosted-adapters/src/console/formats/response.rs
//! `-- hosted-adapters/src/console/formats/stream_in.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! Invariants: same-format bytes remain opaque; malformed parts pass through; buffering is bounded.
//! Contract tests: hosted-adapters/tests/console_formats.rs

#[path = "stream_in.rs"]
mod input;
use super::response::{output_usage, reason};
use super::{Format, FormatError, array, copy, pick, supported, truthy};
use serde_json::{Value, json};

pub fn build_cost_chunk(format: Format, cost: &str) -> String {
    match format {
        Format::Anthropic | Format::Openai => format!(
            "event: ping\ndata: {}\n\n",
            json!({"type":"ping","cost":cost})
        ),
        Format::OaCompat => format!("data: {}\n\n", json!({"choices":[],"cost":cost})),
        _ => format!("data: {}\n\n", json!({"type":"ping","cost":cost})),
    }
}

/// Reproduces createStreamPartConverter, including its first-choice-only output and
/// raw JSON Anthropic output. This is not a complete protocol synthesizer. Usage-only
/// input may pass through unchanged; billing must inspect the original provider parts.
/// Active legacy gateway routes require matching formats, exactly as the TS handler does.
pub fn convert_stream_part(part: &str, from: Format, to: Format) -> Result<String, FormatError> {
    if from == to {
        return Ok(part.to_owned());
    }
    supported(from, to)?;
    let Some(common) = input::from_part(part, from) else {
        return Ok(part.to_owned());
    };
    Ok(to_part(&common, to))
}

fn to_part(chunk: &Value, format: Format) -> String {
    let choices = array(&chunk["choices"]);
    if format == Format::OaCompat {
        let mut out = pick(chunk, &["id", "object", "created", "model"]);
        out["choices"] = json!([]);
        let Some(choice) = choices.first() else {
            return format!("data: {out}");
        };
        let delta = &choice["delta"];
        let mut items = Vec::new();
        for field in ["role", "content"] {
            if truthy(&delta[field]) {
                let mut d = json!({});
                d[field] = delta[field].clone();
                let mut item = pick(choice, &["index"]);
                item["delta"] = d;
                item["finish_reason"] = Value::Null;
                items.push(item);
            }
        }
        for tc in array(&delta["tool_calls"]) {
            let mut item = pick(choice, &["index"]);
            item["delta"] = json!({"tool_calls":[pick(tc,&["index","id","type","function"])]});
            item["finish_reason"] = Value::Null;
            items.push(item);
        }
        if truthy(&choice["finish_reason"]) {
            let mut item = pick(choice, &["index", "finish_reason"]);
            item["delta"] = json!({});
            items.push(item);
        }
        out["choices"] = json!(items);
        if truthy(&chunk["usage"]) {
            out["usage"] = output_usage(&chunk["usage"], format);
        }
        return format!("data: {out}");
    }
    let Some(choice) = choices.first() else {
        return if format == Format::Anthropic {
            "{}".into()
        } else {
            String::new()
        };
    };
    let delta = &choice["delta"];
    if !truthy(delta) {
        return if format == Format::Anthropic {
            "{}".into()
        } else {
            String::new()
        };
    }
    if format == Format::Openai {
        if truthy(&delta["content"]) {
            let mut out = pick(chunk, &["id"]);
            out["type"] = json!("response.output_text.delta");
            out["delta"] = delta["content"].clone();
            out["response"] = pick(chunk, &["id", "model"]);
            return format!("event: response.output_text.delta\ndata: {out}");
        }
        for tc in array(&delta["tool_calls"]) {
            if truthy(&tc["function"]["name"]) {
                let mut item = pick(tc, &["id"]);
                item["type"] = json!("function_call");
                copy(&mut item, "call_id", tc, "id");
                copy(&mut item, "name", &tc["function"], "name");
                item["arguments"] = json!("");
                return format!(
                    "event: response.output_item.added\ndata: {}",
                    json!({"type":"response.output_item.added","output_index":0,"item":item})
                );
            }
            if truthy(&tc["function"]["arguments"]) {
                return format!(
                    "event: response.function_call_arguments.delta\ndata: {}",
                    json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":tc["function"]["arguments"]})
                );
            }
        }
        if truthy(&choice["finish_reason"]) {
            let mut response = pick(chunk, &["id", "model"]);
            if truthy(&chunk["usage"]) {
                response["usage"] = output_usage(&chunk["usage"], format);
            }
            let mut out = pick(chunk, &["id"]);
            out["type"] = json!("response.completed");
            out["response"] = response;
            return format!("event: response.completed\ndata: {out}");
        }
        return String::new();
    }
    let mut out = json!({});
    if truthy(&delta["content"]) {
        out = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":delta["content"]}});
    }
    for tc in array(&delta["tool_calls"]) {
        let index = tc
            .get("index")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(json!(0));
        if truthy(&tc["function"]["name"]) {
            out["type"] = json!("content_block_start");
            out["index"] = index;
            let mut block = pick(tc, &["id"]);
            block["type"] = json!("tool_use");
            copy(&mut block, "name", &tc["function"], "name");
            block["input"] = json!({});
            out["content_block"] = block;
        } else if truthy(&tc["function"]["arguments"]) {
            out["type"] = json!("content_block_delta");
            out["index"] = index;
            out["delta"] =
                json!({"type":"input_json_delta","partial_json":tc["function"]["arguments"]});
        }
    }
    if truthy(&choice["finish_reason"]) {
        out["type"] = json!("message_delta");
        out["delta"] = json!({"stop_reason":reason(&choice["finish_reason"],Format::OaCompat,format),"stop_sequence":null});
    }
    if truthy(&chunk["usage"]) {
        out["usage"] = output_usage(&chunk["usage"], format);
    }
    out.to_string()
}

/// Incremental SSE wrapper. Single-part conversion above intentionally preserves the TS wire quirks.
#[derive(Debug)]
pub struct StreamConverter {
    from: Format,
    to: Format,
    buffer: Vec<u8>,
}
impl StreamConverter {
    pub fn new(from: Format, to: Format) -> Self {
        Self {
            from,
            to,
            buffer: Vec::new(),
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
        if self.from == self.to {
            return Ok(bytes.to_vec());
        }
        supported(self.from, self.to)?;
        self.buffer.extend_from_slice(bytes);
        let mut output = Vec::new();
        loop {
            let boundary = self
                .buffer
                .windows(2)
                .position(|bytes| bytes == b"\n\n")
                .map(|index| (index, 2));
            let crlf = self
                .buffer
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .map(|index| (index, 4));
            let cr = self
                .buffer
                .windows(2)
                .position(|bytes| bytes == b"\r\r")
                .map(|index| (index, 2));
            let Some((index, length)) = boundary
                .into_iter()
                .chain(crlf)
                .chain(cr)
                .min_by_key(|(index, _)| *index)
            else {
                break;
            };
            let part = self.buffer.drain(..index + length).collect::<Vec<_>>();
            let part = std::str::from_utf8(&part[..index])
                .map_err(|error| FormatError::Stream(error.to_string()))?
                .replace("\r\n", "\n");
            let converted = convert_stream_part(part.trim(), self.from, self.to)?;
            if !converted.is_empty() {
                output.extend_from_slice(converted.as_bytes());
                output.extend_from_slice(b"\n\n");
            }
        }
        if self.buffer.len() > 16 * 1024 * 1024 {
            return Err(FormatError::Stream("SSE part exceeds 16 MiB".into()));
        }
        Ok(output)
    }
    pub fn finish(&mut self) -> Result<Vec<u8>, FormatError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = std::mem::take(&mut self.buffer);
        let part = std::str::from_utf8(&bytes)
            .map_err(|error| FormatError::Stream(error.to_string()))?
            .replace("\r\n", "\n");
        let out = convert_stream_part(&part, self.from, self.to)?;
        Ok(if out.is_empty() {
            Vec::new()
        } else {
            format!("{out}\n\n").into_bytes()
        })
    }
}
