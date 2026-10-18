//! Purpose: project completed session replies and tool updates into baseline Slack text.
//! Scope: pure output policy.
//! Uses:
//! rust/crates/
//! `-- ecosystem/src/session.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- ecosystem/src/slack/bot.rs
//! `-- ecosystem/src/slack/thread.rs
//! Invariants: only completed tools produce updates, info.content precedes text parts.
//! Contract tests: ecosystem/tests/slack_bot.rs

use serde_json::Value;

use crate::session::{PromptResponse, SessionEvent};

pub const CREATE_ERROR: &str = "Sorry, I had trouble creating a session. Please try again.";
pub const PROMPT_ERROR: &str = "Sorry, I had trouble processing your message. Please try again.";
pub const EMPTY_RESPONSE: &str = "I received your message but didn't have a response.";
pub const TEST_RESPONSE: &str = "🤖 Bot is working! I can hear you loud and clear.";

pub fn reply(response: &PromptResponse) -> String {
    if let Some(content) = response
        .info
        .get("content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return content.to_string();
    }
    let text = response.joined_text();
    if text.is_empty() {
        EMPTY_RESPONSE.to_string()
    } else {
        text
    }
}

pub fn tool_update(event: &SessionEvent) -> Option<(&str, String)> {
    if event.kind != "message.part.updated" {
        return None;
    }
    let part = event.properties.get("part")?;
    if part.get("type")?.as_str()? != "tool"
        || part.pointer("/state/status")?.as_str()? != "completed"
    {
        return None;
    }
    Some((
        part.get("sessionID")?.as_str()?,
        format!(
            "*{}* - {}",
            part.get("tool")?.as_str()?,
            part.pointer("/state/title")?.as_str()?
        ),
    ))
}
