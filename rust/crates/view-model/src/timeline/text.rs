//! Purpose: text helpers used by timeline rows: reasoning heading extraction, provider error
//! unwrapping and comment-note parsing. Port of `packages/app/src/pages/session/timeline/rows.ts:243-326`
//! and `packages/app/src/utils/comment-note.ts:65-90` (`parseCommentNote`).
//! Scope: pure string functions.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::to_owned
//! `-- str::trim_start
//! Used by:
//! rust/crates/
//! |-- view-model/src/timeline/mod.rs
//! `-- view-model/tests/timeline.rs
//! Invariants: heading precedence is HTML h1-h6, ATX, setext, then a bold-only line; unwrap
//! returns the original message when no JSON error shape is recognized.
//! Contract tests: view-model/tests/timeline.rs

use serde_json::Value;

/// First heading-like line of reasoning Markdown, cleaned of inline markup.
pub fn reasoning_heading(text: &str) -> Option<String> {
    let md = text.replace("\r\n", "\n").replace('\r', "\n");
    html_heading(&md)
        .and_then(clean)
        .or_else(|| md.lines().find_map(atx).and_then(clean))
        .or_else(|| setext(&md).and_then(clean))
        .or_else(|| md.lines().find_map(strong).and_then(clean))
}

fn clean(value: String) -> Option<String> {
    let mut out = String::new();
    let mut rest = value.as_str();
    // `code` -> code, [label](url) -> label
    while let Some(c) = rest.chars().next() {
        if c == '`'
            && let Some(end) = rest[1..].find('`')
            && end > 0
        {
            out.push_str(&rest[1..=end]);
            rest = &rest[end + 2..];
            continue;
        }
        if c == '['
            && let Some(close) = rest.find("](")
            && close > 1
            && let Some(paren) = rest[close + 2..].find(')')
            && paren > 0
        {
            out.push_str(&rest[1..close]);
            rest = &rest[close + 2 + paren + 1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    let out: String = out
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '~'))
        .collect();
    let out = out.trim().to_owned();
    (!out.is_empty()).then_some(out)
}

fn html_heading(md: &str) -> Option<String> {
    let lower = md.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<h") {
        let start = from + i;
        let level = lower.as_bytes().get(start + 2).copied();
        from = start + 2;
        if !matches!(level, Some(b'1'..=b'6')) {
            continue;
        }
        let Some(open_end) = lower[start..].find('>') else {
            continue;
        };
        let body_start = start + open_end + 1;
        let close = format!("</h{}>", level? as char);
        let Some(body_len) = lower[body_start..].find(&close) else {
            continue;
        };
        let body = &md[body_start..body_start + body_len];
        let mut stripped = String::new();
        let mut in_tag = false;
        for c in body.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => {
                    in_tag = false;
                    stripped.push(' ');
                }
                _ if !in_tag => stripped.push(c),
                _ => {}
            }
        }
        return Some(stripped);
    }
    None
}

fn atx(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let body = after
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t']);
    // Optional closing sequence: whitespace then #'s.
    let trimmed = body.trim_end_matches('#');
    let body = if trimmed.len() != body.len() && trimmed.ends_with([' ', '\t']) {
        trimmed.trim_end_matches([' ', '\t'])
    } else {
        body
    };
    (!body.is_empty()).then(|| body.to_owned())
}

fn setext(md: &str) -> Option<String> {
    let lines: Vec<&str> = md.split('\n').collect();
    lines.windows(2).find_map(|w| {
        let under = w[1].trim_end();
        let ok = !w[0].is_empty()
            && !under.is_empty()
            && (under.chars().all(|c| c == '=') || under.chars().all(|c| c == '-'));
        ok.then(|| w[0].to_owned())
    })
}

fn strong(line: &str) -> Option<String> {
    let t = line.trim();
    for mark in ["**", "__"] {
        if t.len() > 4 && t.starts_with(mark) {
            for end in ["**", "__"] {
                if t.ends_with(end) {
                    return Some(t[2..t.len() - 2].to_owned());
                }
            }
        }
    }
    None
}

/// Extract a readable message from provider error text that may embed JSON.
pub fn unwrap_error_message(message: &str) -> String {
    let text = message
        .strip_prefix("Error:")
        .map(str::trim_start)
        .unwrap_or(message)
        .trim();
    let read = |value: &str| -> Option<Value> {
        let first: Value = serde_json::from_str(value).ok()?;
        match first {
            Value::String(s) => serde_json::from_str(s.trim()).ok(),
            other => Some(other),
        }
    };
    let json = read(text).or_else(|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        (end > start).then(|| read(&text[start..=end])).flatten()
    });
    let Some(Value::Object(json)) = json else {
        return message.to_owned();
    };
    let str_of = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    if let Some(Value::Object(err)) = json.get("error") {
        let kind = str_of(err.get("type"));
        let msg = str_of(err.get("message"));
        match (kind, msg) {
            (Some(k), Some(m)) => return format!("{k}: {m}"),
            (None, Some(m)) => return m,
            (Some(k), None) => return k,
            _ => {}
        }
        if let Some(code) = str_of(err.get("code")) {
            return code;
        }
    }
    str_of(json.get("message"))
        .or_else(|| str_of(json.get("error")))
        .unwrap_or_else(|| message.to_owned())
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommentNote {
    pub path: String,
    pub comment: String,
    pub selection: Option<(i64, i64)>,
}

/// Parse `The user made the following comment regarding <range> of <path>: <comment>`.
pub fn parse_comment_note(text: &str) -> Option<CommentNote> {
    let rest = text.strip_prefix("The user made the following comment regarding ")?;
    let (selection, rest) = if let Some(r) = rest.strip_prefix("this file of ") {
        (None, r)
    } else if let Some(r) = rest.strip_prefix("lines ") {
        let (a, r) = r.split_once(" through ")?;
        let (b, r) = r.split_once(" of ")?;
        (Some((a.parse().ok()?, b.parse().ok()?)), r)
    } else {
        let r = rest.strip_prefix("line ")?;
        let (a, r) = r.split_once(" of ")?;
        let n = a.parse().ok()?;
        (Some((n, n)), r)
    };
    let (path, comment) = rest.split_once(": ")?;
    if path.is_empty() || comment.is_empty() {
        return None;
    }
    Some(CommentNote {
        path: path.to_owned(),
        comment: comment.to_owned(),
        selection,
    })
}
