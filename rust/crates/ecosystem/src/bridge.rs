//! Purpose: Feishu event ingress to Discord support-channel projection
//! (`packages/function/src/api.ts:205-258`). Challenge echo, JSON-or-raw content extraction,
//! leading `@_user_N` removal, `aiden` mention rewrite, `[root_id|message_id]` thread suffix,
//! empty-message success and upstream 502. Framework-agnostic, mounted by phase 13.
//! Privacy differences versus baseline: the raw request body is not logged, and
//! the Discord error body is logged only as its length. Ingress authentication
//! (Feishu verification token/signature) is absent in the baseline and remains a blocker.
//! Scope: request.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/github/auth/handlers.rs
//! External:
//! |-- regex::Regex
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::from_str
//! |-- serde_json::json
//! `-- tracing::error
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/lib.rs
//! `-- ecosystem/tests/bridge_feishu.rs
//! Invariants: the bot token is only sent to the configured Discord base URL and never
//! logged, non-JSON request bodies answer 500 like Hono's uncaught `c.req.json()` error.
//! Contract tests: ecosystem/tests/bridge_feishu.rs

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::github::auth::handlers::{HttpRequest, HttpResponse};

pub const DISCORD_API: &str = "https://discord.com/api/v10";
/// Discord user id the baseline rewrites a leading "aiden" to.
pub const AIDEN_MENTION: &str = "<@759257817772851260>";

const JS_WHITESPACE: &str = r"[\u{0009}-\u{000d}\u{0020}\u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
static LEADING_USER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^@_user_[0-9]+{JS_WHITESPACE}*")).expect("static regex")
});
static LEADING_AIDEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)^aiden,?{JS_WHITESPACE}*")).expect("static regex"));

/// Discord destination (`DISCORD_SUPPORT_CHANNEL_ID`, `DISCORD_SUPPORT_BOT_TOKEN`).
#[derive(Clone)]
pub struct FeishuBridge {
    pub discord_base: String,
    pub channel_id: String,
    bot_token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for FeishuBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuBridge")
            .field("discord_base", &self.discord_base)
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

/// Result of transforming one Feishu body, before any Discord call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeishuAction {
    /// URL verification: echo `{ challenge }` with the value unchanged.
    Challenge(Value),
    /// Nothing to post: answer `{ ok: true }`.
    Ignore,
    /// Post this content to Discord.
    Post(String),
    /// Content looked like JSON but did not parse: the baseline `JSON.parse` throws (500).
    Malformed,
}

/// Pure transformation of a parsed Feishu body (`api.ts:220-237`).
pub fn transform(body: &Value) -> FeishuAction {
    if let Some(challenge) = body.get("challenge").filter(|c| truthy(c)) {
        return FeishuAction::Challenge(challenge.clone());
    }
    let message = body.pointer("/event/message");
    let content = message
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str);
    let parsed = match content.filter(|c| js_trim(c).starts_with('{')) {
        Some(c) => match serde_json::from_str::<Value>(c) {
            Ok(value) => Some(value),
            Err(_) => return FeishuAction::Malformed,
        },
        None => None,
    };
    let parsed_text = parsed
        .as_ref()
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str);
    let text = parsed_text.or(content).unwrap_or("").to_string();

    let stripped = LEADING_USER.replace(js_trim(&text), "");
    let mut out = LEADING_AIDEN
        .replace(&stripped, format!("{AIDEN_MENTION} ").as_str())
        .into_owned();
    if out.is_empty() {
        return FeishuAction::Ignore;
    }
    let field = |name: &str| {
        message
            .and_then(|m| m.get(name))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    if let Some(thread) = field("root_id").or_else(|| field("message_id")) {
        out = format!("{out} [{thread}]");
    }
    FeishuAction::Post(out)
}

impl FeishuBridge {
    pub fn new(
        discord_base: impl Into<String>,
        channel_id: impl Into<String>,
        bot_token: impl Into<String>,
    ) -> Self {
        Self {
            discord_base: discord_base.into().trim_end_matches('/').to_string(),
            channel_id: channel_id.into(),
            bot_token: bot_token.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Route `POST /feishu`. Other paths return `None`.
    pub async fn handle(&self, request: &HttpRequest) -> Option<HttpResponse> {
        if request.method != "POST" || request.path != "/feishu" {
            return None;
        }
        Some(self.feishu(&request.body).await)
    }

    pub async fn feishu(&self, raw: &[u8]) -> HttpResponse {
        let Ok(body) = serde_json::from_slice::<Value>(raw) else {
            return HttpResponse::internal_error();
        };
        // JS boxes primitive values for property access, but null throws.
        if body.is_null() {
            return HttpResponse::internal_error();
        }
        match transform(&body) {
            FeishuAction::Challenge(challenge) => {
                HttpResponse::json(200, json!({ "challenge": challenge }))
            }
            FeishuAction::Ignore => HttpResponse::json(200, json!({ "ok": true })),
            FeishuAction::Post(content) => self.post(&content).await,
            FeishuAction::Malformed => HttpResponse::internal_error(),
        }
    }

    async fn post(&self, content: &str) -> HttpResponse {
        let response = self
            .http
            .post(format!(
                "{}/channels/{}/messages",
                self.discord_base, self.channel_id
            ))
            .header("content-type", "application/json")
            .header("authorization", format!("Bot {}", self.bot_token))
            .body(json!({ "content": content }).to_string())
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                HttpResponse::json(200, json!({ "ok": true }))
            }
            Ok(response) => {
                let status = response.status().as_u16();
                let len = response.text().await.map(|t| t.len()).unwrap_or(0);
                tracing::error!(status, body_len = len, "Discord bot message failed");
                HttpResponse::json(502, json!({ "error": "Discord bot message failed" }))
            }
            // A fetch rejection is uncaught in the baseline: Hono default 500.
            Err(_) => HttpResponse::internal_error(),
        }
    }
}

/// JavaScript truthiness for the `if (challenge)` check.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// ECMAScript WhiteSpace and LineTerminator, not Rust's Unicode White_Space.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c| {
        matches!(c,
            '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
            '\u{205f}' | '\u{3000}' | '\u{feff}'
        )
    })
}
