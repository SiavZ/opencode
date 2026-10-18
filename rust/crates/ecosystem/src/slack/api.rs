//! Purpose: Slack Web API calls for Socket Mode URL retrieval, identity and chat output.
//! Scope: per-bot HTTP client with injectable API base and credential-redacted diagnostics.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/slack/mod.rs
//! External:
//! |-- reqwest::Client
//! |-- serde_json::{Value, json}
//! `-- tokio::time
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/slack/bot.rs
//! |-- ecosystem/src/slack/socket.rs
//! `-- ecosystem/src/slack/thread.rs
//! Invariants: credentials are sent only as authorization headers, never logged.
//! Contract tests: ecosystem/tests/slack_bot.rs, ecosystem/tests/slack_socket.rs

use std::time::Duration;

use serde_json::{Value, json};

use super::SlackError;

pub const SLACK_API: &str = "https://slack.com/api";

#[derive(Clone)]
pub struct SlackApi {
    base: String,
    app_token: String,
    bot_token: String,
    http: reqwest::Client,
    retries: u32,
}

impl std::fmt::Debug for SlackApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackApi").finish_non_exhaustive()
    }
}

impl SlackApi {
    pub fn new(
        base: impl Into<String>,
        app_token: impl Into<String>,
        bot_token: impl Into<String>,
    ) -> Self {
        Self {
            base: base.into().trim_end_matches('/').into(),
            app_token: app_token.into(),
            bot_token: bot_token.into(),
            http: reqwest::Client::new(),
            retries: 3,
        }
    }

    pub async fn connection_url(&self) -> Result<String, SlackError> {
        let body = self
            .call("apps.connections.open", &self.app_token, json!({}))
            .await?;
        body.get("url")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or(SlackError::Protocol("missing Socket Mode URL"))
    }

    /// Bound Web API retries. Zero is useful for typed failure fixtures.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries.min(10);
        self
    }

    /// Bolt's default ignoreSelf middleware needs auth.test's user identity.
    pub async fn bot_user_id(&self) -> Result<String, SlackError> {
        let body = self.call("auth.test", &self.bot_token, json!({})).await?;
        body.get("user_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(SlackError::Protocol("missing bot user identity"))
    }

    pub async fn post_message(
        &self,
        channel: &str,
        thread: Option<&str>,
        text: &str,
    ) -> Result<(), SlackError> {
        self.post_optional_message(channel, thread, Some(text))
            .await
    }

    /// Preserve Bolt's omitted text when a successful share response has no URL.
    pub async fn post_optional_message(
        &self,
        channel: &str,
        thread: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), SlackError> {
        let mut body = json!({ "channel": channel });
        if let Some(text) = text {
            body["text"] = json!(text);
        }
        if let Some(thread) = thread {
            body["thread_ts"] = json!(thread);
        }
        self.call("chat.postMessage", &self.bot_token, body).await?;
        Ok(())
    }

    async fn call(&self, method: &str, token: &str, body: Value) -> Result<Value, SlackError> {
        // Keep individual HTTP waits bounded. The bot additionally bounds session work
        // and cancels every in-flight operation when its owner shuts down.
        let mut attempt = 0u32;
        loop {
            let response = self
                .http
                .post(format!("{}/{method}", self.base))
                .bearer_auth(token)
                .json(&body)
                .timeout(Duration::from_secs(30))
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) if attempt >= self.retries => {
                    return Err(SlackError::Transport(error.without_url().to_string()));
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100) * 2u32.pow(attempt)).await;
                    attempt += 1;
                    continue;
                }
            };
            let status = response.status().as_u16();
            if status == 429 {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                if attempt >= self.retries || retry_after > 30 {
                    return Err(SlackError::RateLimited { retry_after });
                }
                drop(response);
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                attempt += 1;
                continue;
            }
            if status >= 500 && attempt < self.retries {
                drop(response);
                tokio::time::sleep(Duration::from_millis(100) * 2u32.pow(attempt)).await;
                attempt += 1;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(SlackError::Http { status });
            }
            let value: Value = response
                .json()
                .await
                .map_err(|_| SlackError::Protocol("invalid Slack API JSON"))?;
            if value.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(SlackError::Api {
                    code: value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown_error")
                        .to_string(),
                });
            }
            return Ok(value);
        }
    }
}
