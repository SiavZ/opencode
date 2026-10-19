//! Purpose: render specialized tool metadata without confusing pending or failed states.
//! Scope: tool body projection; navigation and expansion events remain app-owned.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/timeline/diff.rs
//! |-- ui-terminal/src/timeline/mod.rs
//! `-- ui-terminal/src/timeline/tool.rs
//! External:
//! |-- ratatui::{style, text}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/timeline/tool.rs
//! Invariants: malformed metadata is ignored, not interpreted as successful work.
//! Contract tests: ui-terminal/tests/tool_widgets.rs
use super::{
    diff, safe_text,
    tool::{Status, Tool, display_metadata},
};
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use serde_json::Value;

pub(super) fn render(tool: &Tool) -> Option<Vec<Line<'static>>> {
    let metadata = display_metadata(tool);
    let get = |key| metadata.and_then(|value| value.get(key));
    match tool.name.as_str() {
        "read" => Some(if tool.status == Status::Completed {
            get("loaded")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|path| Line::raw(format!("↳ Loaded {}", safe_text(path))))
                .collect()
        } else {
            Vec::new()
        }),
        "write" if get("diagnostics").is_some() => {
            let mut rows = tool.input["content"]
                .as_str()
                .unwrap_or_default()
                .split('\n')
                .enumerate()
                .map(|(index, line)| Line::raw(format!("{:>3} {}", index + 1, safe_text(line))))
                .collect::<Vec<_>>();
            rows.extend(diagnostics(
                get("diagnostics"),
                tool.input["filePath"].as_str().unwrap_or_default(),
            ));
            Some(rows)
        }
        "edit" => {
            let mut rows = get("diff")
                .and_then(Value::as_str)
                .map(diff::render_patch)
                .unwrap_or_default();
            rows.extend(diagnostics(
                get("diagnostics"),
                tool.input["filePath"].as_str().unwrap_or_default(),
            ));
            Some(rows)
        }
        "apply_patch" => Some(
            get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|file| {
                    let Some(kind) = file["type"].as_str() else {
                        return Vec::new();
                    };
                    let Some(path) = file["relativePath"].as_str() else {
                        return Vec::new();
                    };
                    let Some(patch) = file["patch"].as_str() else {
                        return Vec::new();
                    };
                    let Some(deletions) = file["deletions"].as_u64() else {
                        return Vec::new();
                    };
                    let title = match kind {
                        "delete" => format!("# Deleted {path}"),
                        "add" => format!("# Created {path}"),
                        "move" => format!(
                            "# Moved {} → {path}",
                            file["filePath"].as_str().unwrap_or_default()
                        ),
                        _ => format!("← Patched {path}"),
                    };
                    let mut rows = vec![Line::raw(safe_text(&title))];
                    if kind == "delete" {
                        rows.push(Line::styled(
                            format!("-{deletions} line{}", if deletions == 1 { "" } else { "s" }),
                            Style::default().fg(Color::Red),
                        ));
                    } else {
                        rows.extend(diff::render_patch(patch));
                    }
                    rows.extend(diagnostics(
                        get("diagnostics"),
                        file["movePath"]
                            .as_str()
                            .or_else(|| file["filePath"].as_str())
                            .unwrap_or_default(),
                    ));
                    rows
                })
                .collect(),
        ),
        "todowrite" => Some(
            if get("todos")
                .and_then(Value::as_array)
                .is_some_and(|todos| !todos.is_empty())
            {
                tool.input["todos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|todo| {
                        let status = todo["status"].as_str()?;
                        let content = todo["content"].as_str()?;
                        let mark = match status {
                            "completed" => "[x]",
                            "in_progress" => "[•]",
                            "cancelled" => "[-]",
                            _ => "[ ]",
                        };
                        Some(Line::raw(format!("{mark} {}", safe_text(content))))
                    })
                    .collect()
            } else {
                Vec::new()
            },
        ),
        "question" => Some(
            get("answers")
                .and_then(Value::as_array)
                .map(|answers| {
                    tool.input["questions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .filter_map(|(index, question)| {
                            let question = question["question"].as_str()?;
                            let answer = answers
                                .get(index)
                                .and_then(Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            Some(vec![
                                Line::raw(safe_text(question)),
                                Line::raw(if answer.is_empty() {
                                    "(no answer)".into()
                                } else {
                                    safe_text(&answer)
                                }),
                            ])
                        })
                        .flatten()
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "execute" => Some(
            get("toolCalls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|call| {
                    let name = call["tool"].as_str()?;
                    let status = call["status"].as_str()?;
                    if !["running", "completed", "error"].contains(&status) {
                        return None;
                    }
                    Some(Line::styled(
                        format!(
                            "↳ {}{}",
                            safe_text(name),
                            if status == "error" { " (failed)" } else { "" }
                        ),
                        Style::default().fg(if status == "error" {
                            Color::Red
                        } else {
                            Color::DarkGray
                        }),
                    ))
                })
                .collect(),
        ),
        _ => None,
    }
}
fn diagnostics(value: Option<&Value>, path: &str) -> Vec<Line<'static>> {
    value
        .and_then(|value| value.get(path))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|diagnostic| {
            if diagnostic["severity"].as_u64() != Some(1) {
                return None;
            }
            let line = diagnostic["range"]["start"]["line"].as_u64()?;
            let character = diagnostic["range"]["start"]["character"].as_u64()?;
            let message = diagnostic["message"].as_str()?;
            Some(Line::styled(
                format!(
                    "Error [{}:{}] {}",
                    line.saturating_add(1),
                    character.saturating_add(1),
                    safe_text(message)
                ),
                Style::default().fg(Color::Red),
            ))
        })
        .take(3)
        .collect()
}
