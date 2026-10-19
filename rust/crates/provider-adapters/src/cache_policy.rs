//! Purpose: apply `LlmRequest.cache` by injecting `CacheHint`s at policy boundaries
//! (packages/llm/src/cache-policy.ts). Manual hints are preserved; only gaps are filled.
//! Scope: pure request transform.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- provider-adapters/src/route.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/lib.rs
//! |-- provider-adapters/src/protocols/anthropic/body.rs
//! |-- provider-adapters/src/protocols/bedrock/body.rs
//! `-- provider-adapters/src/route.rs
//! Invariants: only routes `anthropic-messages` and `bedrock-converse` receive hints. Unset
//! policy means `auto` (last tool, last system part, latest user message).
//! Contract tests: provider-adapters/src/cache_policy.rs (unit tests)

use opencode_llm_api::{
    CacheHint, CacheHintType, CacheMessagesPolicy, CachePolicy, CachePolicyObject, ContentPart,
    LlmRequest, Message, MessageRole,
};

/// Route ids whose wire format honours inline cache markers.
pub const RESPECTS_INLINE_HINTS: &[&str] = &["anthropic-messages", "bedrock-converse"];

fn resolve(policy: Option<&CachePolicy>) -> CachePolicyObject {
    match policy {
        None => auto(),
        Some(CachePolicy::Named(name)) if name == "auto" => auto(),
        Some(CachePolicy::Named(_)) => CachePolicyObject::default(),
        Some(CachePolicy::Object(object)) => object.clone(),
    }
}

fn auto() -> CachePolicyObject {
    CachePolicyObject {
        tools: Some(true),
        system: Some(true),
        messages: Some(CacheMessagesPolicy::Named("latest-user-message".into())),
        ttl_seconds: None,
    }
}

/// `applyCachePolicy`.
pub fn apply_cache_policy(route_id: &str, mut request: LlmRequest) -> LlmRequest {
    if !RESPECTS_INLINE_HINTS.contains(&route_id) {
        return request;
    }
    let policy = resolve(request.cache.as_ref());
    let hint = CacheHint {
        kind: CacheHintType::Ephemeral,
        ttl_seconds: policy.ttl_seconds,
    };
    if policy.tools == Some(true)
        && let Some(last) = request.tools.last_mut()
        && last.cache.is_none()
    {
        last.cache = Some(hint.clone());
    }
    if policy.system == Some(true)
        && let Some(last) = request.system.last_mut()
        && last.cache.is_none()
    {
        last.cache = Some(hint.clone());
    }
    match policy.messages {
        Some(CacheMessagesPolicy::Named(name)) if name == "latest-user-message" => {
            mark_last_role(&mut request.messages, MessageRole::User, &hint)
        }
        Some(CacheMessagesPolicy::Named(name)) if name == "latest-assistant" => {
            mark_last_role(&mut request.messages, MessageRole::Assistant, &hint)
        }
        Some(CacheMessagesPolicy::Tail { tail }) => {
            let start = request.messages.len().saturating_sub(tail as usize);
            for message in &mut request.messages[start..] {
                mark_message(message, &hint);
            }
        }
        _ => {}
    }
    request
}

fn mark_last_role(messages: &mut [Message], role: MessageRole, hint: &CacheHint) {
    if let Some(message) = messages.iter_mut().rev().find(|m| m.role == role) {
        mark_message(message, hint);
    }
}

/// Mark the last text part, or the last part of any type when no text part exists.
fn mark_message(message: &mut Message, hint: &CacheHint) {
    let Some(index) = message
        .content
        .iter()
        .rposition(|p| matches!(p, ContentPart::Text { .. }))
        .or_else(|| message.content.len().checked_sub(1))
    else {
        return;
    };
    match &mut message.content[index] {
        ContentPart::Text { cache, .. } if cache.is_none() => *cache = Some(hint.clone()),
        ContentPart::ToolResult(part) if part.cache.is_none() => part.cache = Some(hint.clone()),
        // Media, tool-call and reasoning parts carry no cache field in the baseline schema,
        // the JS spread adds one that no protocol reads. Nothing to record here.
        _ => {}
    }
}

/// Anthropic and Bedrock cap at 4 breakpoints and accept `5m`/`1h` buckets (protocols/utils/cache.ts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoints {
    pub remaining: u32,
    pub dropped: u32,
}

impl Breakpoints {
    pub fn new(cap: u32) -> Self {
        Self {
            remaining: cap,
            dropped: 0,
        }
    }

    /// Consume one breakpoint, counting drops once the cap is reached.
    pub fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.dropped += 1;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

/// `ttlBucket`: `"1h"` for ttl >= 3600 seconds, else provider default.
pub fn ttl_bucket(ttl_seconds: Option<f64>) -> Option<&'static str> {
    ttl_seconds.filter(|t| *t >= 3600.0).map(|_| "1h")
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencode_llm_api::{Model, SystemPart};

    fn request() -> LlmRequest {
        let mut r = LlmRequest::new(Model::new(
            "m",
            "p",
            crate::route::tests_support::dummy_route(),
        ));
        r.system = vec![SystemPart::text("a"), SystemPart::text("b")];
        r.messages = vec![
            Message::user("u1"),
            Message::assistant(vec![ContentPart::text("x")]),
            Message::user("u2"),
        ];
        r
    }

    fn cached(part: &ContentPart) -> bool {
        matches!(part, ContentPart::Text { cache: Some(_), .. })
    }

    #[test]
    fn auto_marks_last_system_and_latest_user() {
        let r = apply_cache_policy("anthropic-messages", request());
        assert!(r.system[0].cache.is_none());
        assert!(r.system[1].cache.is_some());
        assert!(!cached(&r.messages[0].content[0]));
        assert!(cached(&r.messages[2].content[0]));
    }

    #[test]
    fn other_routes_and_none_are_untouched() {
        let r = apply_cache_policy("openai-chat", request());
        assert!(r.system[1].cache.is_none());
        let mut req = request();
        req.cache = Some(CachePolicy::Named("none".into()));
        let r = apply_cache_policy("bedrock-converse", req);
        assert!(r.system[1].cache.is_none());
    }

    #[test]
    fn tail_marks_trailing_messages_with_ttl() {
        let mut req = request();
        req.cache = Some(CachePolicy::Object(CachePolicyObject {
            messages: Some(CacheMessagesPolicy::Tail { tail: 2 }),
            ttl_seconds: Some(3600.0),
            ..Default::default()
        }));
        let r = apply_cache_policy("anthropic-messages", req);
        assert!(!cached(&r.messages[0].content[0]));
        assert!(cached(&r.messages[1].content[0]) && cached(&r.messages[2].content[0]));
        assert_eq!(ttl_bucket(Some(3600.0)), Some("1h"));
        assert_eq!(ttl_bucket(Some(300.0)), None);
    }
}
