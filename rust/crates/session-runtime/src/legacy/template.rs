//! Purpose: pure text helpers of the legacy prompt paths: `ConfigMarkdown.files/shell` matchers
//! (packages/opencode/src/config/markdown.ts:5-14), command argument splitting and `$N`/`$ARGUMENTS`
//! templating (prompt.ts:1356-1398, 1592-1596), `Command.hints` (command/index.ts:36-44),
//! `Provider.parseModel`, `decodeDataUrl`, `Session.isDefaultTitle` and JS `String.replace`
//! replacement-pattern semantics.
//! Scope: pure functions, no I/O.
//! Uses:
//! `-- none
//! External:
//! `-- base64::Engine
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/command.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/title.rs
//! `-- session-runtime/tests/legacy_command.rs
//! Invariants: matchers reproduce the JS regexes exactly, including the `(?<![\w`])` lookbehind of
//! FILE_REGEX (ASCII `\w`), greedy dotted-segment capture and global-match resumption.
//! Contract tests: session-runtime/tests/legacy_command.rs

use base64::Engine;

fn is_word(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_sep(ch: char) -> bool {
    ch.is_whitespace() || ch == '`' || ch == ',' || ch == '.'
}

/// `ConfigMarkdown.files(template)`: captured names of `@path` references, in order.
pub fn file_references(template: &str) -> Vec<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '@'
            || (index > 0 && (is_word(chars[index - 1]) || chars[index - 1] == '`'))
        {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        if chars.get(end) == Some(&'.') {
            end += 1;
        }
        while end < chars.len() && !is_sep(chars[end]) {
            end += 1;
        }
        // `(?:\.[^\s`,.]+)*`: each segment needs a dot and at least one non-separator.
        while chars.get(end) == Some(&'.') && chars.get(end + 1).is_some_and(|ch| !is_sep(*ch)) {
            end += 1;
            while end < chars.len() && !is_sep(chars[end]) {
                end += 1;
            }
        }
        result.push(chars[index + 1..end].iter().collect());
        index = end;
    }
    result
}

/// `ConfigMarkdown.shell(template)`: byte ranges and commands of each `` !`cmd` ``.
pub fn shell_references(template: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut result = Vec::new();
    let mut from = 0;
    while let Some(offset) = template[from..].find("!`") {
        let start = from + offset;
        let body = start + 2;
        match template[body..].find('`') {
            Some(0) | None => from = start + 1,
            Some(len) => {
                result.push((start..body + len + 1, template[body..body + len].to_owned()));
                from = body + len + 1;
            }
        }
    }
    result
}

/// Replaces each `` !`cmd` `` with the matching entry of `outputs` (`template.replace(bashRegex, ..)`).
pub fn replace_shell(template: &str, outputs: &[String]) -> String {
    let mut result = String::new();
    let mut last = 0;
    for ((range, _), output) in shell_references(template).into_iter().zip(outputs) {
        result.push_str(&template[last..range.start]);
        result.push_str(output);
        last = range.end;
    }
    result.push_str(&template[last..]);
    result
}

/// `input.arguments.match(argsRegex)` then quote trimming.
pub fn split_arguments(arguments: &str) -> Vec<String> {
    let chars: Vec<char> = arguments.chars().collect();
    let mut raw = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if let Some(end) = image_token(&chars, index) {
            raw.push(chars[index..end].iter().collect::<String>());
            index = end;
            continue;
        }
        let ch = chars[index];
        if ch == '"' || ch == '\'' {
            if let Some(close) = chars[index + 1..].iter().position(|c| *c == ch) {
                let end = index + 1 + close + 1;
                raw.push(chars[index..end].iter().collect());
                index = end;
                continue;
            }
            index += 1;
            continue;
        }
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && chars[index] != '"'
            && chars[index] != '\''
        {
            index += 1;
        }
        raw.push(chars[start..index].iter().collect());
    }
    raw.into_iter().map(|arg| trim_quotes(&arg)).collect()
}

/// `\[Image\s+\d+\]` (case-insensitive) at `index`.
fn image_token(chars: &[char], index: usize) -> Option<usize> {
    let word: String = chars.get(index..index + 6)?.iter().collect();
    if !word.eq_ignore_ascii_case("[image") {
        return None;
    }
    let mut end = index + 6;
    let spaces = chars[end..]
        .iter()
        .take_while(|c| c.is_whitespace())
        .count();
    if spaces == 0 {
        return None;
    }
    end += spaces;
    let digits = chars[end..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    end += digits;
    (chars.get(end) == Some(&']')).then_some(end + 1)
}

/// `arg.replace(/^["']|["']$/g, "")`.
fn trim_quotes(arg: &str) -> String {
    let start = usize::from(arg.starts_with(['"', '\'']));
    let rest = &arg[start..];
    rest.strip_suffix(['"', '\'']).unwrap_or(rest).to_owned()
}

/// Positions of `$N` placeholders: `(byte range, N)`.
fn placeholders(template: &str) -> Vec<(std::ops::Range<usize>, u64)> {
    let bytes = template.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            let digits = bytes[index + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 0 {
                let end = index + 1 + digits;
                let value = template[index + 1..end].parse::<f64>().unwrap_or(0.0) as u64;
                result.push((index..end, value));
                index = end;
                continue;
            }
        }
        index += 1;
    }
    result
}

/// The command template expansion of `SessionPrompt.command` before shell substitution.
pub fn expand_command(template: &str, arguments: &str) -> String {
    let args = split_arguments(arguments);
    let found = placeholders(template);
    let last = found.iter().map(|(_, value)| *value).max().unwrap_or(0);
    let mut with_args = String::new();
    let mut cursor = 0;
    for (range, position) in &found {
        with_args.push_str(&template[cursor..range.start]);
        if *position == 0 {
            // `argIndex = -1`: `-1 >= length` is false, so `$0` is `args.slice(-1).join(" ")` when it
            // is the largest placeholder, else `args[-1]` (`undefined`, rendered as "undefined").
            if last == 0 {
                with_args.push_str(args.last().map_or("", String::as_str));
            } else {
                with_args.push_str("undefined");
            }
        } else if (*position as usize - 1) >= args.len() {
        } else if *position == last {
            with_args.push_str(&args[*position as usize - 1..].join(" "));
        } else {
            with_args.push_str(&args[*position as usize - 1]);
        }
        cursor = range.end;
    }
    with_args.push_str(&template[cursor..]);
    let uses_arguments = template.contains("$ARGUMENTS");
    let mut result = replace_all_js(&with_args, "$ARGUMENTS", arguments);
    if found.is_empty() && !uses_arguments && !arguments.trim().is_empty() {
        result = format!("{result}\n\n{arguments}");
    }
    result
}

/// JS `String.prototype.replaceAll(pattern: string, replacement: string)` including the
/// `$$`, `$&`, `` $` `` and `$'` replacement patterns.
pub fn replace_all_js(input: &str, pattern: &str, replacement: &str) -> String {
    if pattern.is_empty() {
        return input.to_owned();
    }
    let mut result = String::new();
    let mut last = 0;
    for (start, matched) in input.match_indices(pattern) {
        result.push_str(&input[last..start]);
        result.push_str(&substitute(
            replacement,
            matched,
            &input[..start],
            &input[start + matched.len()..],
        ));
        last = start + matched.len();
    }
    result.push_str(&input[last..]);
    result
}

/// `GetSubstitution` without capture groups.
fn substitute(replacement: &str, matched: &str, before: &str, after: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                out.push_str(matched);
            }
            Some('`') => {
                chars.next();
                out.push_str(before);
            }
            Some('\'') => {
                chars.next();
                out.push_str(after);
            }
            _ => out.push('$'),
        }
    }
    out
}

/// JS `String.prototype.replace(pattern: string, replacement: string)` (first occurrence).
pub fn replace_first_js(input: &str, pattern: &str, replacement: &str) -> String {
    match input.find(pattern) {
        Some(start) => {
            let end = start + pattern.len();
            format!(
                "{}{}{}",
                &input[..start],
                substitute(replacement, pattern, &input[..start], &input[end..]),
                &input[end..]
            )
        }
        None => input.to_owned(),
    }
}

/// `Command.hints(template)`.
pub fn hints(template: &str) -> Vec<String> {
    let mut numbered: Vec<String> = placeholders(template)
        .into_iter()
        .map(|(range, _)| template[range].to_owned())
        .collect();
    numbered.sort();
    numbered.dedup();
    if template.contains("$ARGUMENTS") {
        numbered.push("$ARGUMENTS".into());
    }
    numbered
}

/// `Provider.parseModel("provider/model/with/slashes")`.
pub fn parse_model(model: &str) -> (String, String) {
    match model.split_once('/') {
        Some((provider, rest)) => (provider.to_owned(), rest.to_owned()),
        None => (model.to_owned(), String::new()),
    }
}

/// `decodeDataUrl`.
pub fn decode_data_url(url: &str) -> String {
    let Some(index) = url.find(',') else {
        return String::new();
    };
    let (head, body) = (&url[..index], &url[index + 1..]);
    if head.contains(";base64") {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body.trim_end_matches('='))
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD_NO_PAD.decode(body.trim_end_matches('='))
            })
            .unwrap_or_default();
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    percent_decode(body)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = input.get(index + 1..index + 3)
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `Session.isDefaultTitle`: `New session - ` or `Child session - ` plus an ISO timestamp
/// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn is_default_title(title: &str) -> bool {
    let Some(stamp) = title
        .strip_prefix("New session - ")
        .or_else(|| title.strip_prefix("Child session - "))
    else {
        return false;
    };
    let shape = "dddd-dd-ddTdd:dd:dd.dddZ";
    stamp.len() == shape.len()
        && stamp.bytes().zip(shape.bytes()).all(|(ch, want)| {
            if want == b'd' {
                ch.is_ascii_digit()
            } else {
                ch == want
            }
        })
}
