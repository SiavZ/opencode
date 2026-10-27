//! Purpose: sanitize legacy session exports with the baseline's field-specific redactions.
//! Scope: pure JSON transformation, retaining structural and accounting fields.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/sessions.rs
//! Invariants: blank text stays blank; IDs and timing remain unchanged; no source fields leak.
//! Contract tests: opencode/tests/admin/mod.rs

use serde_json::{Value, json};

pub fn sanitize(value: &mut Value) {
    let info = &mut value["info"];
    let id = info["id"].as_str().unwrap_or_default().to_owned();
    redact(info, "title", "session-title", &id);
    redact(info, "directory", "session-directory", &id);
    if let Some(summary) = info.get_mut("summary") {
        diffs(summary.get_mut("diffs"), "session-diff");
    }
    if let Some(revert) = info.get_mut("revert") {
        redact(revert, "snapshot", "revert-snapshot", &id);
        redact(revert, "diff", "revert-diff", &id);
    }
    for message in value["messages"].as_array_mut().into_iter().flatten() {
        let info = &mut message["info"];
        let id = info["id"].as_str().unwrap_or_default().to_owned();
        if info["role"] == "user" {
            redact(info, "system", "system", &id);
            if let Some(summary) = info.get_mut("summary") {
                redact(summary, "title", "summary-title", &id);
                redact(summary, "body", "summary-body", &id);
                diffs(summary.get_mut("diffs"), "message-diff");
            }
        } else if let Some(path) = info.get_mut("path") {
            redact(path, "cwd", "cwd", &id);
            redact(path, "root", "root", &id);
        }
        for part in message["parts"].as_array_mut().into_iter().flatten() {
            sanitize_part(part);
        }
    }
}

fn sanitize_part(part: &mut Value) {
    let id = part["id"].as_str().unwrap_or_default().to_owned();
    match part["type"].as_str().unwrap_or_default() {
        "text" | "reasoning" => {
            let kind = part["type"].as_str().unwrap().to_owned();
            redact(part, "text", &kind, &id);
            data(part, "metadata", &format!("{kind}-metadata"), &id);
        }
        "file" => file(part, &id),
        "subtask" => {
            redact(part, "prompt", "subtask-prompt", &id);
            redact(part, "description", "subtask-description", &id);
            redact(part, "command", "subtask-command", &id);
        }
        "tool" => {
            data(part, "metadata", "tool-metadata", &id);
            let state = &mut part["state"];
            data(state, "input", "tool-input", &id);
            match state["status"].as_str().unwrap_or_default() {
                "pending" => redact(state, "raw", "tool-raw", &id),
                "running" => {
                    redact(state, "title", "tool-title", &id);
                    data(state, "metadata", "tool-state-metadata", &id);
                }
                "completed" => {
                    redact(state, "output", "tool-output", &id);
                    redact(state, "title", "tool-title", &id);
                    data(state, "metadata", "tool-state-metadata", &id);
                    for item in state
                        .get_mut("attachments")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        let id = item["id"].as_str().unwrap_or_default().to_owned();
                        file(item, &id);
                    }
                }
                _ => data(state, "metadata", "tool-state-metadata", &id),
            }
        }
        "patch" => {
            redact(part, "hash", "patch", &id);
            for (index, value) in part["files"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .enumerate()
            {
                text(value, "patch-file", &format!("{id}-{index}"));
            }
        }
        "snapshot" => redact(part, "snapshot", "snapshot", &id),
        "step-start" | "step-finish" => redact(part, "snapshot", "snapshot", &id),
        "agent" => {
            if let Some(source) = part.get_mut("source") {
                redact(source, "value", "agent-source", &id);
            }
        }
        _ => {}
    }
}

fn file(part: &mut Value, id: &str) {
    redact(part, "url", "file-url", id);
    redact(part, "filename", "file-name", id);
    if let Some(source) = part.get_mut("source") {
        match source["type"].as_str().unwrap_or_default() {
            "symbol" => {
                redact(source, "path", "file-path", id);
                redact(source, "name", "file-symbol", id);
            }
            "resource" => {
                redact(source, "clientName", "file-client", id);
                redact(source, "uri", "file-uri", id);
            }
            _ => redact(source, "path", "file-path", id),
        }
        if let Some(span) = source.get_mut("text") {
            redact(span, "value", "file-text", id);
        }
    }
}

fn diffs(value: Option<&mut Value>, kind: &str) {
    for (index, item) in value
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .enumerate()
    {
        redact(item, "file", &format!("{kind}-file"), &index.to_string());
        redact(item, "patch", &format!("{kind}-patch"), &index.to_string());
    }
}

fn redact(value: &mut Value, key: &str, kind: &str, id: &str) {
    if let Some(value) = value.get_mut(key) {
        text(value, kind, id);
    }
}

fn text(value: &mut Value, kind: &str, id: &str) {
    if value.as_str().is_some_and(|text| !text.trim().is_empty()) {
        *value = json!(format!("[redacted:{kind}:{id}]"));
    }
}

fn data(value: &mut Value, key: &str, kind: &str, id: &str) {
    if let Some(value) = value.get_mut(key)
        && value.as_object().is_some_and(|object| !object.is_empty())
    {
        *value = json!({"redacted":format!("{kind}:{id}")});
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;
    use serde_json::json;

    #[test]
    fn nested_export_redactions_preserve_accounting_and_blank_values() {
        let mut value = json!({
            "info":{"id":"ses_fixture","title":"private","directory":"/private","time":{"created":7},"summary":{"diffs":[{"file":"private.rs","patch":"secret","additions":2}]}},
            "messages":[{"info":{"id":"msg_fixture","role":"assistant","path":{"cwd":"/private","root":"/root"}},"parts":[
                {"id":"prt_blank","type":"text","text":" \n","metadata":{}},
                {"id":"prt_tool","type":"tool","state":{"status":"completed","input":{"secret":true},"title":"private","output":"private","time":{"start":3,"end":4},"metadata":{"secret":true},"attachments":[{"id":"prt_file","type":"file","url":"file:///private","source":{"type":"symbol","path":"/private","name":"private","text":{"value":"private","start":0,"end":7}}}]}},
                {"id":"prt_error","type":"tool","state":{"status":"error","input":{},"error":"baseline keeps this error","metadata":{"secret":true}}},
                {"id":"prt_patch","type":"patch","hash":"private","files":["private.rs"]}
            ]}]
        });
        sanitize(&mut value);
        assert_eq!(value["info"]["time"]["created"], 7);
        assert_eq!(
            value["info"]["summary"]["diffs"][0]["file"],
            "[redacted:session-diff-file:0]"
        );
        assert_eq!(value["info"]["summary"]["diffs"][0]["additions"], 2);
        let parts = &value["messages"][0]["parts"];
        assert_eq!(parts[0]["text"], " \n");
        assert_eq!(parts[0]["metadata"], json!({}));
        assert_eq!(
            parts[1]["state"]["input"],
            json!({"redacted":"tool-input:prt_tool"})
        );
        assert_eq!(parts[1]["state"]["time"], json!({"start":3,"end":4}));
        assert_eq!(
            parts[1]["state"]["attachments"][0]["source"]["text"],
            json!({"value":"[redacted:file-text:prt_file]","start":0,"end":7})
        );
        assert_eq!(parts[2]["state"]["error"], "baseline keeps this error");
        assert_eq!(parts[3]["files"][0], "[redacted:patch-file:prt_patch-0]");
    }
}
