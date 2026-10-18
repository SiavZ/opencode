//! Purpose: classify provider error text as context-window overflow (packages/llm/src/provider-error.ts
//! `isContextOverflow`). Drives `InvalidRequest.classification = "context-overflow"`.
//! Scope: pure regex classification, no I/O.
//! Uses:
//! `-- none
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/transport/executor.rs
//! `-- provider-adapters/src/transport/mod.rs
//! Invariants: pattern list, exclusion list and the `4xx (no body)` fallback match the baseline
//! order and case-insensitivity exactly. Exclusions (rate limits, throttling, service
//! unavailable) always win.
//! Contract tests: provider-adapters/tests/transport_executor.rs, provider-adapters/src/transport/overflow.rs (unit tests)

use std::sync::LazyLock;

use regex::Regex;

/// Baseline `patterns` (provider-error.ts), each compiled case-insensitive.
const PATTERNS: &[&str] = &[
    r"prompt is too long",
    r"request_too_large",
    r"input is too long for requested model",
    r"exceeds the context window",
    r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))",
    r"input token count.*exceeds the maximum",
    r"tokens in request more than max tokens allowed",
    r"maximum prompt length is \d+",
    r"reduce the length of the messages",
    r"maximum context length is \d+ tokens",
    r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?",
    r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
    r"exceeds the limit of \d+",
    r"exceeds the available context size",
    r"greater than the context length",
    r"context window exceeds limit",
    r"exceeded model token limit",
    r"context[_ ]length[_ ]exceeded",
    r"request entity too large",
    r"context length is only \d+ tokens",
    r"input length.*exceeds.*context length",
    r"prompt too long; exceeded (?:max )?context length",
    r"too large for model with \d+ maximum context length",
    r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?",
    r"model_context_window_exceeded",
    r"too many tokens",
    r"token limit exceeded",
];

const EXCLUSIONS: &[&str] = &[
    r"^(throttling error|service unavailable):",
    r"rate limit",
    r"too many requests",
];

const NO_BODY: &str = r"^4(00|13)\s*(status code)?\s*\(no body\)";

fn compile(sources: &[&str]) -> Vec<Regex> {
    sources
        .iter()
        .map(|s| Regex::new(&format!("(?i){s}")).expect("static overflow pattern"))
        .collect()
}

static PATTERN_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| compile(PATTERNS));
static EXCLUSION_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| compile(EXCLUSIONS));
static NO_BODY_RE: LazyLock<Regex> = LazyLock::new(|| compile(&[NO_BODY]).remove(0));

/// `isContextOverflow(message)`.
pub fn is_context_overflow(message: &str) -> bool {
    !EXCLUSION_RES.iter().any(|re| re.is_match(message))
        && (PATTERN_RES.iter().any(|re| re.is_match(message)) || NO_BODY_RE.is_match(message))
}

#[cfg(test)]
mod tests {
    use super::is_context_overflow;

    #[test]
    fn baseline_examples() {
        for text in [
            r#"{"error":{"code":"context_length_exceeded","message":"prompt too long"}}"#,
            "prompt is too long: 210000 tokens > 200000 maximum",
            "This model's maximum context length is 128000 tokens",
            "Input token count 1048577 exceeds the maximum",
            "400 (no body)",
            "413 status code (no body)",
            "exceeds the model's maximum context length of 8,192 tokens",
            "Prompt has 9,000 tokens, but the configured context size is 8,192 tokens",
        ] {
            assert!(is_context_overflow(text), "{text}");
        }
        for text in [
            "request too large",
            "invalid parameter",
            "rate limit: too many tokens",
            "Throttling error: too many tokens",
            "Too Many Requests: token limit exceeded",
            "404 (no body)",
        ] {
            assert!(!is_context_overflow(text), "{text}");
        }
    }
}
