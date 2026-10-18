//! Purpose: opaque pagination cursors of the current API: `SessionsCursor`
//! (`packages/protocol/src/groups/session.ts`) and the message cursor
//! (`packages/server/src/handlers/message.ts`).
//! Scope: pure encode/decode.
//! Uses:
//! rust/crates/
//! `-- schema/src/session.rs
//! External:
//! |-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- server/src/handlers/session.rs
//! Invariants: both cursors are unpadded base64url of compact JSON. Sessions cursor keys follow
//! the schema order `workspace, order, search, directory | project, subpath, anchor` and omit
//! absent keys; `limit` is never encoded. Decoding accepts padded and unpadded input like
//! `Buffer.from(_, "base64url")` and Effect `decodeBase64UrlString`.
//! Contract tests: server/src/handlers/cursor.rs (unit), server/tests/http_session.rs

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use opencode_schema::session::ListAnchor;
use serde::{Deserialize, Serialize};

/// Decoded sessions cursor (the query it continues plus its anchor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionsCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    pub anchor: ListAnchor,
}

impl SessionsCursor {
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default())
    }

    /// `SessionsCursor.parse`; `None` maps to `InvalidCursorError("Invalid cursor")`.
    pub fn parse(input: &str) -> Option<Self> {
        let bytes = decode_base64url(input)?;
        let cursor: SessionsCursor = serde_json::from_slice(&bytes).ok()?;
        if cursor
            .order
            .as_deref()
            .is_some_and(|o| o != "asc" && o != "desc")
        {
            return None;
        }
        if cursor
            .workspace
            .as_deref()
            .is_some_and(|w| !w.starts_with("wrk"))
        {
            return None;
        }
        Some(cursor)
    }
}

/// Message paging cursor `{ id, order, direction }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageCursor {
    pub id: String,
    pub order: String,
    pub direction: String,
}

impl MessageCursor {
    pub fn encode(id: &str, order: &str, direction: &str) -> String {
        let cursor = MessageCursor {
            id: id.to_string(),
            order: order.to_string(),
            direction: direction.to_string(),
        };
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap_or_default())
    }

    pub fn decode(input: &str) -> Option<Self> {
        let cursor: MessageCursor = serde_json::from_slice(&decode_base64url(input)?).ok()?;
        let valid = cursor.id.starts_with("msg_")
            && matches!(cursor.order.as_str(), "asc" | "desc")
            && matches!(cursor.direction.as_str(), "previous" | "next");
        valid.then_some(cursor)
    }
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(input.trim_end_matches('=')).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_cursor_round_trips_unpadded() {
        let encoded = MessageCursor::encode("msg_1", "desc", "next");
        assert!(!encoded.contains('='));
        // fixture: Buffer.from('{"id":"msg_1","order":"desc","direction":"next"}').toString("base64url")
        assert_eq!(
            encoded,
            "eyJpZCI6Im1zZ18xIiwib3JkZXIiOiJkZXNjIiwiZGlyZWN0aW9uIjoibmV4dCJ9"
        );
        let decoded = MessageCursor::decode(&encoded).unwrap();
        assert_eq!(decoded.id, "msg_1");
        assert!(MessageCursor::decode("not base64 !").is_none());
    }

    #[test]
    fn sessions_cursor_rejects_garbage() {
        assert!(SessionsCursor::parse("%%%").is_none());
        assert!(SessionsCursor::parse(&URL_SAFE_NO_PAD.encode("{}")).is_none());
    }
}
