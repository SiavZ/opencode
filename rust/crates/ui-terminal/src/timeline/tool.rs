//! Purpose: project tool lifecycle, structured titles, output and errors into rows.
//! Scope: pure widget rendering.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/timeline/mod.rs
//! `-- ui-terminal/src/timeline/tool_details.rs
//! External:
//! |-- ratatui::{style, text}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! Invariants: pending metadata is not exposed; failures are never styled as success.
//! Contract tests: ui-terminal/tests/rendering.rs
use super::safe_text;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Completed,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub status: Status,
    pub title: Option<String>,
    pub input: Value,
    pub structured: Value,
    pub output: String,
    pub error: Option<String>,
    pub expanded: bool,
}
pub fn web_search_provider_label(provider: Option<&str>) -> &'static str {
    match provider {
        Some("parallel") => "Parallel Web Search",
        Some("exa") => "Exa Web Search",
        _ => "Web Search",
    }
}
pub fn display_metadata(tool: &Tool) -> Option<&serde_json::Map<String, Value>> {
    if tool.status == Status::Pending {
        return None;
    }
    tool.structured.as_object()
}
pub fn render(tool: &Tool) -> Vec<Line<'static>> {
    let (mark, color) = match tool.status {
        Status::Pending => ("○", Color::DarkGray),
        Status::Running => ("◉", Color::Yellow),
        Status::Completed => ("✓", Color::Green),
        Status::Error => ("✗", Color::Red),
    };
    let title = tool.title.as_deref().unwrap_or_else(|| {
        if tool.name == "websearch" {
            return web_search_provider_label(
                display_metadata(tool)
                    .and_then(|m| m.get("provider"))
                    .and_then(Value::as_str),
            );
        }
        &tool.name
    });
    let detail = [
        "command",
        "filePath",
        "path",
        "pattern",
        "query",
        "url",
        "description",
    ]
    .into_iter()
    .find_map(|key| tool.input.get(key).and_then(Value::as_str))
    .unwrap_or("");
    let mut lines = vec![Line::styled(
        format!("{mark} {} {}", safe_text(title), safe_text(detail)),
        Style::default().fg(color),
    )];
    if let Some(error) = &tool.error {
        lines.extend(
            safe_text(error)
                .lines()
                .map(|line| Line::styled(line.to_owned(), Style::default().fg(Color::Red))),
        );
    }
    if let Some(details) = super::tool_details::render(tool) {
        lines.extend(details);
        return lines;
    }
    if tool.expanded {
        lines.extend(
            safe_text(&tool.output)
                .split('\n')
                .map(|line| Line::raw(line.to_owned())),
        );
    }
    if !tool.expanded && !tool.output.is_empty() {
        let count = tool.output.lines().count();
        lines.extend(
            safe_text(&tool.output)
                .lines()
                .take(3)
                .map(|line| Line::raw(line.to_owned())),
        );
        if count > 3 {
            lines.push(Line::styled(
                format!("… {} more lines", count - 3),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    lines
}
