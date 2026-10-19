//! Purpose: decode V2 and legacy durable message snapshots into renderable parts.
//! Scope: pure JSON boundary, with typed errors for malformed or unknown content.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/timeline/mod.rs
//! `-- ui-terminal/src/timeline/tool.rs
//! External:
//! |-- serde_json::Value
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! Invariants: unknown lifecycle states fail instead of appearing pending or successful.
//! Contract tests: ui-terminal/tests/timeline_projection.rs
use super::{
    Message, Part,
    tool::{Status, Tool},
};
use serde_json::Value;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProjectionError {
    #[error("invalid timeline field: {0}")]
    Invalid(&'static str),
    #[error("unsupported timeline content: {0}")]
    Unsupported(String),
}
impl Message {
    pub fn from_value(value: &Value) -> Result<Self, ProjectionError> {
        let info = value.get("info").unwrap_or(value);
        let role = info
            .get("role")
            .or_else(|| info.get("type"))
            .and_then(Value::as_str)
            .ok_or(ProjectionError::Invalid("role/type"))?;
        let mut parts = Vec::new();
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            parts.push(Part::Text(text.to_owned()));
        }
        if let Some(content) = value.get("content").or_else(|| value.get("parts")) {
            let content = content
                .as_array()
                .ok_or(ProjectionError::Invalid("content/parts"))?;
            parts.extend(
                content
                    .iter()
                    .map(part)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten(),
            );
        }
        match role {
            "assistant" | "user" | "system" | "synthetic" => {}
            "shell" => parts.push(Part::Tool(Tool {
                name: "bash".into(),
                status: if info["time"].get("completed").is_some() {
                    Status::Completed
                } else {
                    Status::Running
                },
                title: None,
                input: serde_json::json!({"command": value["command"]}),
                structured: Value::Null,
                output: value["output"].as_str().unwrap_or_default().into(),
                error: None,
                expanded: false,
            })),
            "agent-switched" => parts.push(Part::Text(format!(
                "Agent: {}",
                value["agent"]
                    .as_str()
                    .ok_or(ProjectionError::Invalid("agent"))?
            ))),
            "model-switched" => parts.push(Part::Text(format!(
                "Model: {}/{}",
                value["model"]["providerID"]
                    .as_str()
                    .ok_or(ProjectionError::Invalid("model.providerID"))?,
                value["model"]["id"]
                    .as_str()
                    .ok_or(ProjectionError::Invalid("model.id"))?
            ))),
            "compaction" => parts.push(Part::Text(format!(
                "Compacted: {}",
                value["summary"]
                    .as_str()
                    .ok_or(ProjectionError::Invalid("summary"))?
            ))),
            _ => return Err(ProjectionError::Unsupported(role.into())),
        }
        if let Some(error) = value["error"]
            .as_str()
            .or_else(|| value["error"]["message"].as_str())
        {
            parts.push(Part::Text(format!("Error: {error}")));
        }
        if let Some(files) = value["files"].as_array() {
            parts.extend(files.iter().map(|file| {
                Part::Text(format!(
                    "[File: {}]",
                    file["filename"]
                        .as_str()
                        .or_else(|| file["uri"].as_str())
                        .unwrap_or("attachment")
                ))
            }));
        }
        Ok(Self {
            role: role.into(),
            parts,
        })
    }
}
fn part(value: &Value) -> Result<Option<Part>, ProjectionError> {
    match value["type"]
        .as_str()
        .ok_or(ProjectionError::Invalid("part.type"))?
    {
        "text" => Ok(Some(Part::Text(
            value["text"]
                .as_str()
                .ok_or(ProjectionError::Invalid("part.text"))?
                .into(),
        ))),
        "reasoning" => Ok(Some(Part::Reasoning(
            value["text"]
                .as_str()
                .ok_or(ProjectionError::Invalid("part.text"))?
                .into(),
        ))),
        "tool" => {
            let state = &value["state"];
            let status = match state["status"].as_str() {
                Some("pending") => Status::Pending,
                Some("running") => Status::Running,
                Some("completed") => Status::Completed,
                Some("error") => Status::Error,
                Some(other) => {
                    return Err(ProjectionError::Unsupported(format!("tool status {other}")));
                }
                None => return Err(ProjectionError::Invalid("tool.state.status")),
            };
            let structured = state
                .get("metadata")
                .or_else(|| state.get("structured"))
                .cloned()
                .unwrap_or(Value::Null);
            let output = state["output"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    state["content"]
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                });
            Ok(Some(Part::Tool(Tool {
                name: value
                    .get("tool")
                    .or_else(|| value.get("name"))
                    .and_then(Value::as_str)
                    .ok_or(ProjectionError::Invalid("tool.name"))?
                    .into(),
                status,
                title: state["title"]
                    .as_str()
                    .or_else(|| structured["title"].as_str())
                    .map(str::to_owned),
                input: state["input"].clone(),
                structured,
                output,
                error: state["error"]
                    .as_str()
                    .or_else(|| state["error"]["message"].as_str())
                    .map(str::to_owned),
                expanded: false,
            })))
        }
        "file" => Ok(Some(Part::Text(format!(
            "[File: {}]",
            value["filename"]
                .as_str()
                .or_else(|| value["url"].as_str())
                .unwrap_or("attachment")
        )))),
        "step-start" | "step-finish" | "snapshot" | "patch" => Ok(None),
        other => Err(ProjectionError::Unsupported(other.into())),
    }
}
