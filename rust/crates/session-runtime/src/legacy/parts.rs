//! Purpose: `SessionPrompt.createUserMessage` part resolution (packages/opencode/src/session/
//! prompt.ts:635-1053) and `resolvePromptParts` (:157-191): file/data/MCP-resource/agent parts,
//! synthetic Read-tool transcript text, directory reads, binary inlining and image normalization.
//! MCP resource parts live in `legacy/resource.rs`.
//! Scope: one user message; I/O through `PromptTools` and `opencode_platform_api::fs::FileSystem`.
//! Uses:
//! rust/crates/
//! |-- permissions/src/wildcard.rs
//! |-- platform-api/src/fs.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/ports.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/template.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- base64::Engine
//! |-- char::is_ascii_digit
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- url::Url
//! |-- url::Url::from_file_path
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! `-- session-runtime/src/legacy/resource.rs
//! Invariants: synthetic text wording, MCP binary limits (10 MiB, pdf/gif/jpeg/png/webp) and part key
//! order match the baseline. Read failures publish `session.error` and degrade to a synthetic text
//! part instead of failing the prompt. Each resolved part gets an ascending `prt` ID unless it had one.
//! Contract tests: session-runtime/tests/legacy_prompt.rs

use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{Value, json};

use super::LegacySession;
use super::ports::ReadContext;
use super::store::object;
use super::template::{decode_data_url, file_references};
use super::types::{AgentInfo, LegacyError, Object, part_id, str_field, unknown_error};

/// JS `JSON.stringify` of a Read-tool args object (absent fields omitted).
fn read_args(path: &str, offset: Option<i64>, limit: Option<i64>) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("filePath".into(), json!(path));
    if let Some(offset) = offset {
        args.insert("offset".into(), json!(offset));
    }
    if let Some(limit) = limit {
        args.insert("limit".into(), json!(limit));
    }
    Value::Object(args)
}

/// JS `parseInt(text)`: leading integer digits, NaN as `None`.
fn parse_int(text: &str) -> Option<i64> {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<i64>().ok().map(|value| sign * value)
}

pub(super) struct Draft<'a> {
    pub session: &'a str,
    pub message: &'a str,
    pub agent: &'a AgentInfo,
    pub input_agent: Option<&'a str>,
    pub model: (&'a str, &'a str),
}

impl Draft<'_> {
    pub(super) fn synthetic(&self, text: impl Into<String>) -> Object {
        object(json!({
            "messageID": self.message,
            "sessionID": self.session,
            "type": "text",
            "synthetic": true,
            "text": text.into(),
        }))
    }

    /// `{ ...part, messageID, sessionID }` keeping the input key order.
    fn spread(&self, part: &Object, mime: Option<&str>) -> Object {
        let mut out = part.clone();
        if let Some(mime) = mime {
            out.insert("mime".into(), json!(mime));
        }
        out.insert("messageID".into(), json!(self.message));
        out.insert("sessionID".into(), json!(self.session));
        out
    }
}

impl LegacySession {
    /// `resolvePromptParts(template)`: the text plus deduplicated `@file`/`@agent` references.
    pub async fn resolve_prompt_parts(&self, template: &str) -> Result<Vec<Object>, LegacyError> {
        let mut parts = vec![object(json!({ "type": "text", "text": template }))];
        let mut seen = std::collections::HashSet::new();
        for name in file_references(template) {
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }
            let path = self.resolve_path(&name);
            match self.deps.fs.metadata(&path).await {
                Ok(meta) => {
                    let mime = if meta.file_type == opencode_platform_api::fs::FileType::Directory {
                        "application/x-directory"
                    } else {
                        "text/plain"
                    };
                    parts.push(object(json!({
                        "type": "file",
                        "url": file_url(&path),
                        "filename": name,
                        "mime": mime,
                    })));
                }
                Err(_) => {
                    if let Some(found) = self.deps.agents.get(&name).await? {
                        parts.push(object(json!({ "type": "agent", "name": found.name })));
                    }
                }
            }
        }
        Ok(parts)
    }

    fn resolve_path(&self, name: &str) -> PathBuf {
        if let Some(rest) = name.strip_prefix("~/") {
            return self.deps.instance.home.join(rest);
        }
        normalize(&self.deps.instance.worktree.join(name))
    }

    /// `resolvePart` for one input part.
    pub(super) async fn resolve_part(
        &self,
        draft: &Draft<'_>,
        part: &Object,
    ) -> Result<Vec<Object>, LegacyError> {
        match str_field(part, "type") {
            Some("file") => self.resolve_file(draft, part).await,
            Some("agent") => {
                let name = str_field(part, "name").unwrap_or_default();
                let denied = evaluate("task", name, &draft.agent.permission) == "deny";
                let hint = if denied {
                    " . Invoked by user; guaranteed to exist."
                } else {
                    ""
                };
                Ok(vec![
                    draft.spread(part, None),
                    draft.synthetic(format!(
                        " Use the above message and context to generate a prompt and call the task tool with subagent: {name}{hint}"
                    )),
                ])
            }
            _ => Ok(vec![draft.spread(part, None)]),
        }
    }

    async fn resolve_file(
        &self,
        draft: &Draft<'_>,
        part: &Object,
    ) -> Result<Vec<Object>, LegacyError> {
        let source = part.get("source").and_then(Value::as_object);
        if source.and_then(|s| str_field(s, "type")) == Some("resource") {
            return self
                .resolve_resource(draft, part, source.cloned().unwrap_or_default())
                .await;
        }
        let url = str_field(part, "url").unwrap_or_default();
        let parsed =
            url::Url::parse(url).map_err(|_| LegacyError::Unknown("Invalid URL".into()))?;
        let mime = str_field(part, "mime").unwrap_or_default();
        match parsed.scheme() {
            "data" if mime == "text/plain" => Ok(vec![
                draft.synthetic(format!(
                    "Called the Read tool with the following input: {}",
                    json!({ "filePath": part.get("filename") })
                )),
                draft.synthetic(decode_data_url(url)),
                draft.spread(part, None),
            ]),
            "file" => self.resolve_local(draft, part, &parsed).await,
            _ => Ok(vec![draft.spread(part, None)]),
        }
    }

    async fn resolve_local(
        &self,
        draft: &Draft<'_>,
        part: &Object,
        url: &url::Url,
    ) -> Result<Vec<Object>, LegacyError> {
        let path = url
            .to_file_path()
            .map_err(|_| LegacyError::Unknown("File URL path must be absolute".into()))?;
        let filepath = path.to_string_lossy().into_owned();
        let is_dir = self
            .deps
            .fs
            .metadata(&path)
            .await
            .is_ok_and(|meta| meta.file_type == opencode_platform_api::fs::FileType::Directory);
        let mime = if is_dir {
            "application/x-directory"
        } else {
            str_field(part, "mime").unwrap_or_default()
        };
        let ctx = ReadContext {
            session_id: draft.session.into(),
            message_id: draft.message.into(),
            agent: draft.input_agent.map(str::to_owned),
            model: None,
        };
        if mime == "text/plain" {
            let (offset, limit) = self.range(url, part).await;
            let args = read_args(&filepath, offset, limit);
            let mut pieces = vec![draft.synthetic(format!(
                "Called the Read tool with the following input: {args}"
            ))];
            let model = self
                .deps
                .provider
                .get_model(draft.model.0, draft.model.1)
                .await;
            let result = match model {
                Ok(model) => self
                    .deps
                    .tools
                    .read(
                        args,
                        ReadContext {
                            model: Some(model),
                            ..ctx
                        },
                    )
                    .await
                    .map_err(LegacyError::from),
                Err(error) => Err(error),
            };
            match result {
                Ok(read) => {
                    pieces.push(draft.synthetic(read.output));
                    if read.attachments.is_empty() {
                        pieces.push(draft.spread(part, Some(mime)));
                    } else {
                        pieces.extend(read.attachments.into_iter().map(|mut attachment| {
                            attachment.insert("synthetic".into(), json!(true));
                            if attachment.get("filename").is_none_or(Value::is_null) {
                                attachment.insert(
                                    "filename".into(),
                                    part.get("filename").cloned().unwrap_or(Value::Null),
                                );
                            }
                            attachment.insert("messageID".into(), json!(draft.message));
                            attachment.insert("sessionID".into(), json!(draft.session));
                            attachment
                        }));
                    }
                }
                Err(error) => pieces.push(self.read_failed(draft, &filepath, error).await?),
            }
            return Ok(pieces);
        }
        if mime == "application/x-directory" {
            let args = read_args(&filepath, None, None);
            return match self.deps.tools.read(args.clone(), ctx).await {
                Ok(read) => Ok(vec![
                    draft.synthetic(format!(
                        "Called the Read tool with the following input: {args}"
                    )),
                    draft.synthetic(read.output),
                    draft.spread(part, Some(mime)),
                ]),
                Err(error) => Ok(vec![
                    self.read_failed(draft, &filepath, error.into()).await?,
                ]),
            };
        }
        let bytes = self
            .deps
            .fs
            .read(&path)
            .await
            .map_err(|error| LegacyError::Unknown(error.to_string()))?;
        let mut file = object(json!({
            "id": part.get("id"),
            "messageID": draft.message,
            "sessionID": draft.session,
            "type": "file",
            "url": format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)),
            "mime": mime,
            "filename": part.get("filename"),
            "source": part.get("source"),
        }));
        file.retain(|_, value| !value.is_null());
        Ok(vec![
            draft.synthetic(format!(
                "Called the Read tool with the following input: {{\"filePath\":\"{filepath}\"}}"
            )),
            file,
        ])
    }

    /// `?start=&end=` handling, with LSP symbol lookup when `start === end`.
    async fn range(&self, url: &url::Url, part: &Object) -> (Option<i64>, Option<i64>) {
        let query = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        let Some(start_text) = query("start") else {
            return (None, None);
        };
        let mut start = parse_int(&start_text);
        let mut end = query("end")
            .filter(|e| !e.is_empty())
            .and_then(|e| parse_int(&e));
        if start.is_some() && start == end {
            let uri = str_field(part, "url").unwrap_or_default();
            let uri = uri.split('?').next().unwrap_or_default();
            let symbols = self
                .deps
                .tools
                .document_symbols(uri)
                .await
                .unwrap_or_default();
            for symbol in symbols {
                let range = symbol
                    .get("range")
                    .or_else(|| symbol.get("location").and_then(|l| l.get("range")));
                let line = range
                    .and_then(|r| r["start"]["line"].as_i64())
                    .filter(|l| *l != 0);
                if line.is_some() && line == start {
                    start = line;
                    end = range.and_then(|r| r["end"]["line"].as_i64()).or(start);
                    break;
                }
            }
        }
        // `Math.max(NaN, 1)` is NaN: an unparsable start leaves offset NaN (serialized as null).
        let offset = start.map(|start| start.max(1));
        let limit = match (end, offset) {
            (Some(end), Some(offset)) if end != 0 => Some(end - (offset - 1)),
            _ => None,
        };
        (offset, limit)
    }

    async fn read_failed(
        &self,
        draft: &Draft<'_>,
        filepath: &str,
        error: LegacyError,
    ) -> Result<Object, LegacyError> {
        if let LegacyError::NotYetPorted(_) = error {
            return Err(error);
        }
        let message = error.to_string();
        self.deps
            .store
            .publish_error(draft.session, unknown_error(&message))
            .await?;
        Ok(draft.synthetic(format!(
            "Read tool failed to read {filepath} with the following error: {message}"
        )))
    }

    /// `assign`: keeps a given part ID, else an ascending `prt` ID.
    pub(super) fn assign_ids(parts: Vec<Object>) -> Vec<Object> {
        parts
            .into_iter()
            .map(|mut part| {
                let id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map_or_else(part_id, str::to_owned);
                part.insert("id".into(), json!(id));
                part
            })
            .collect()
    }

    /// `image.normalize` for image file parts (after the `chat.message` hook).
    pub(super) async fn normalize_images(
        &self,
        parts: Vec<Object>,
    ) -> Result<Vec<Object>, LegacyError> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            let image = str_field(&part, "type") == Some("file")
                && str_field(&part, "mime").is_some_and(|mime| mime.starts_with("image/"));
            if image && let Some(normalized) = self.deps.tools.normalize_image(&part).await? {
                out.push(normalized);
                continue;
            }
            out.push(part);
        }
        Ok(out)
    }
}

/// `PermissionV1.evaluate(permission, pattern, ruleset).action`: last matching rule, else `ask`.
pub(super) fn evaluate(permission: &str, pattern: &str, rules: &[Object]) -> String {
    rules
        .iter()
        .rev()
        .find(|rule| {
            opencode_permissions::wildcard::matches(
                permission,
                str_field(rule, "permission").unwrap_or_default(),
            ) && opencode_permissions::wildcard::matches(
                pattern,
                str_field(rule, "pattern").unwrap_or_default(),
            )
        })
        .and_then(|rule| str_field(rule, "action"))
        .unwrap_or("ask")
        .to_owned()
}

/// `pathToFileURL(path).href`.
pub(super) fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// `path.resolve` normalization of `.`/`..` segments without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
