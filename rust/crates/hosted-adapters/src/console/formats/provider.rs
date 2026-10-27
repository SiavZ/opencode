//! Purpose: configure native Zen provider HTTP requests and normalize billing usage.
//! Scope: OpenAI, chat-completions, Anthropic/Bedrock/Databricks, Google and Systemone helpers.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! External:
//! |-- http::HeaderMap
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/formats.rs
//! Invariants: token subtraction, optional cache fields and streamed usage match baseline helpers.
//! Contract tests: hosted-adapters/tests/console_formats.rs

use super::{Format, FormatError, truthy};
use http::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageInfo {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write5m_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write1h_tokens: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ProviderHelper {
    pub format: Format,
    req_model: String,
    provider_model: String,
    adjust_cache_usage: bool,
}

impl ProviderHelper {
    pub fn new(
        format: Format,
        req_model: &str,
        provider_model: &str,
        adjust_cache_usage: bool,
    ) -> Self {
        Self {
            format,
            req_model: req_model.into(),
            provider_model: provider_model.into(),
            adjust_cache_usage,
        }
    }
    pub fn is_bedrock(&self) -> bool {
        self.format == Format::Anthropic
            && (self.provider_model.starts_with("arn:aws:bedrock:")
                || self.provider_model.starts_with("global.anthropic."))
    }
    fn supports_1m(&self) -> bool {
        self.req_model.contains("sonnet") || self.req_model.contains("opus-4-6")
    }
    pub fn modify_url(&self, api: &str, stream: bool) -> String {
        match self.format {
            Format::Openai => format!("{api}/responses"),
            Format::OaCompat => format!("{api}/chat/completions"),
            Format::Anthropic if self.is_bedrock() => {
                let model = if self.provider_model.starts_with("arn:") {
                    // encodeURIComponent leaves these punctuation characters unescaped.
                    self.provider_model
                        .bytes()
                        .map(|byte| {
                            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                                (byte as char).to_string()
                            } else {
                                format!("%{byte:02X}")
                            }
                        })
                        .collect::<String>()
                } else {
                    self.provider_model.clone()
                };
                format!(
                    "{api}/model/{model}/{}",
                    if stream {
                        "invoke-with-response-stream"
                    } else {
                        "invoke"
                    }
                )
            }
            Format::Anthropic => format!("{api}/messages"),
            Format::Google => format!(
                "{api}/models/{}:{}",
                self.provider_model,
                if stream {
                    "streamGenerateContent?alt=sse"
                } else {
                    "generateContent"
                }
            ),
            Format::Systemone => format!("{}/systemone", api.strip_suffix('/').unwrap_or(api)),
        }
    }
    pub fn modify_headers(
        &self,
        headers: &mut HeaderMap,
        api_key: &str,
        sticky_id: &str,
    ) -> Result<(), FormatError> {
        match self.format {
            Format::Anthropic
                if !self.is_bedrock() && !self.provider_model.starts_with("databricks-claude-") =>
            {
                headers.insert("x-api-key", HeaderValue::from_str(api_key)?);
                if !headers.contains_key("anthropic-version") {
                    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
                }
                if self.supports_1m() {
                    headers.insert(
                        "anthropic-beta",
                        HeaderValue::from_static("context-1m-2025-08-07"),
                    );
                }
            }
            Format::Google => {
                headers.insert("x-goog-api-key", HeaderValue::from_str(api_key)?);
            }
            _ => {
                headers.insert(
                    "authorization",
                    HeaderValue::from_str(&format!("Bearer {api_key}"))?,
                );
                if matches!(self.format, Format::OaCompat | Format::Systemone) {
                    headers.insert("x-session-affinity", HeaderValue::from_str(sticky_id)?);
                }
            }
        }
        Ok(())
    }
    pub fn modify_body(&self, mut body: Value) -> Value {
        if !body.is_object() {
            return body;
        }
        if self.format == Format::OaCompat && truthy(&body["stream"]) {
            body["stream_options"] = json!({"include_usage":true});
        }
        if self.format == Format::Anthropic
            && (self.is_bedrock() || self.provider_model.starts_with("databricks-claude-"))
        {
            body["anthropic_version"] = json!("bedrock-2023-05-31");
            if self.supports_1m() {
                body["anthropic_beta"] = json!(["context-1m-2025-08-07"]);
            } else {
                body.as_object_mut().unwrap().remove("anthropic_beta");
            }
            if self.is_bedrock() {
                let object = body.as_object_mut().unwrap();
                object.remove("model");
                object.remove("stream");
            }
        }
        body
    }
    pub fn extract_usage(&self, response: &Value) -> Option<Value> {
        let usage = if self.format == Format::Google {
            response.get("usageMetadata")
        } else {
            response
                .get("usage")
                .filter(|value| !value.is_null())
                .or_else(|| {
                    (self.format == Format::Openai)
                        .then(|| response.pointer("/response/usage"))
                        .flatten()
                })
        };
        usage.filter(|value| !value.is_null()).cloned()
    }
    pub fn normalize_usage(&self, usage: &Value) -> UsageInfo {
        let n = |path: &str| usage.pointer(path).and_then(Value::as_i64);
        let mut result = UsageInfo::default();
        match self.format {
            Format::Openai => {
                result.cache_read_tokens = n("/input_tokens_details/cached_tokens");
                result.cache_write5m_tokens = n("/input_tokens_details/cache_write_tokens");
                result.input_tokens = (n("/input_tokens").unwrap_or(0)
                    - result.cache_read_tokens.unwrap_or(0)
                    - result.cache_write5m_tokens.unwrap_or(0))
                .max(0);
                result.output_tokens = n("/output_tokens").unwrap_or(0);
                result.reasoning_tokens = n("/output_tokens_details/reasoning_tokens");
            }
            Format::OaCompat => {
                let input = n("/prompt_tokens").unwrap_or(0);
                result.cache_read_tokens =
                    n("/cached_tokens").or_else(|| n("/prompt_tokens_details/cached_tokens"));
                if self.adjust_cache_usage && result.cache_read_tokens.unwrap_or(0) == 0 {
                    result.cache_read_tokens = Some((input as f64 * 0.9).floor() as i64);
                }
                result.input_tokens = input - result.cache_read_tokens.unwrap_or(0);
                result.output_tokens = n("/completion_tokens").unwrap_or(0);
                result.reasoning_tokens = n("/completion_tokens_details/reasoning_tokens");
                result.cache_write5m_tokens =
                    n("/prompt_tokens_details/cache_creation_input_tokens");
            }
            Format::Anthropic => {
                result.input_tokens = n("/input_tokens").unwrap_or(0);
                result.output_tokens = n("/output_tokens").unwrap_or(0);
                result.cache_read_tokens = n("/cache_read_input_tokens");
                result.cache_write5m_tokens = n("/cache_creation/ephemeral_5m_input_tokens")
                    .or_else(|| n("/cache_creation_input_tokens"));
                result.cache_write1h_tokens = n("/cache_creation/ephemeral_1h_input_tokens");
            }
            Format::Google => {
                result.cache_read_tokens = Some(n("/cachedContentTokenCount").unwrap_or(0));
                result.reasoning_tokens = Some(n("/thoughtsTokenCount").unwrap_or(0));
                result.input_tokens =
                    n("/promptTokenCount").unwrap_or(0) - result.cache_read_tokens.unwrap_or(0);
                result.output_tokens =
                    n("/candidatesTokenCount").unwrap_or(0) + result.reasoning_tokens.unwrap_or(0);
            }
            Format::Systemone => {
                result.input_tokens = n("/input_tokens").unwrap_or(0);
                result.output_tokens = n("/output_tokens").unwrap_or(0);
            }
        }
        result
    }
}

#[derive(Debug)]
pub struct UsageParser {
    format: Format,
    usage: Option<Value>,
}
impl UsageParser {
    pub fn new(format: Format) -> Self {
        Self {
            format,
            usage: None,
        }
    }
    pub fn retrieve(&self) -> Option<&Value> {
        self.usage.as_ref()
    }
    pub fn parse(&mut self, part: &str) {
        let mut lines = part.split('\n');
        let first = lines.next().unwrap_or_default();
        let data = match self.format {
            Format::Systemone => return,
            Format::Openai if first == "event: response.completed" => {
                lines.next().and_then(|line| line.strip_prefix("data: "))
            }
            Format::Openai => return,
            Format::Anthropic => lines
                .next()
                .and_then(|line| line.strip_prefix("data:"))
                .map(str::trim_start),
            _ => first.strip_prefix("data: "),
        };
        let Some(json) = data.and_then(|data| serde_json::from_str::<Value>(data).ok()) else {
            return;
        };
        let update = match self.format {
            Format::Openai => json.pointer("/response/usage"),
            Format::Google => json.get("usageMetadata"),
            Format::Anthropic => json
                .get("usage")
                .filter(|value| truthy(value))
                .or_else(|| json.pointer("/message/usage")),
            _ => json.get("usage"),
        };
        let Some(update) = update.filter(|value| truthy(value)) else {
            return;
        };
        if self.format != Format::Anthropic {
            self.usage = Some(update.clone());
            return;
        }
        let mut merged = self.usage.take().unwrap_or_else(|| json!({}));
        for (key, value) in update.as_object().into_iter().flatten() {
            if key != "cache_creation" && key != "server_tool_use" {
                merged[key] = value.clone();
            }
        }
        for key in ["cache_creation", "server_tool_use"] {
            let mut nested = merged[key]
                .as_object()
                .cloned()
                .map(Value::Object)
                .unwrap_or_else(|| json!({}));
            for (field, value) in update[key].as_object().into_iter().flatten() {
                nested[field] = value.clone();
            }
            merged[key] = nested;
        }
        self.usage = Some(merged);
    }
}
