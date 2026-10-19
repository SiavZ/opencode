//! Purpose: project message parts into styled, display-width-safe terminal rows.
//! Scope: per session timeline, no IO.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/timeline/diff.rs
//! |-- ui-terminal/src/timeline/markdown.rs
//! |-- ui-terminal/src/timeline/projection.rs
//! |-- ui-terminal/src/timeline/tool.rs
//! `-- ui-terminal/src/timeline/tool_details.rs
//! External:
//! |-- ratatui::{style, text}
//! |-- unicode_segmentation::UnicodeSegmentation
//! `-- unicode_width::UnicodeWidthStr
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
//! Invariants: streaming deltas replace only their own message part; clusters never split.
//! Contract tests: ui-terminal/tests/rendering.rs
pub mod diff;
pub mod markdown;
mod projection;
pub use projection::ProjectionError;
pub mod tool;
mod tool_details;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Reasoning(String),
    Diff { before: String, after: String },
    Tool(tool::Tool),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub parts: Vec<Part>,
}
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    messages: Vec<Message>,
}
impl Timeline {
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
    pub fn clear(&mut self) {
        self.messages.clear();
    }
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }
    pub fn push(&mut self, role: impl Into<String>, text: impl Into<String>) {
        self.messages.push(Message {
            role: role.into(),
            parts: vec![Part::Text(text.into())],
        });
    }
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }
    /// The app starts a new message with push, then feeds provider deltas here.
    pub fn append(&mut self, role: &str, delta: &str) {
        if let Some(Message {
            role: current,
            parts,
        }) = self.messages.last_mut()
            && current == role
        {
            if let Some(Part::Text(text)) = parts.last_mut() {
                text.push_str(delta);
                return;
            }
            parts.push(Part::Text(delta.to_owned()));
            return;
        }
        self.push(role, delta);
    }
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        self.messages
            .iter()
            .flat_map(|message| {
                let mut lines = vec![Line::styled(
                    safe_text(&message.role),
                    Style::default().fg(Color::Cyan),
                )];
                lines.extend(message.parts.iter().flat_map(|part| {
                    match part {
                        Part::Text(text) => markdown::render(text),
                        Part::Reasoning(text) => markdown::render(text)
                            .into_iter()
                            .map(|line| line.style(Style::default().fg(Color::DarkGray)))
                            .collect(),
                        Part::Diff { before, after } => diff::render(before, after),
                        Part::Tool(tool) => tool::render(tool),
                    }
                }));
                lines.push(Line::default());
                lines
                    .into_iter()
                    .flat_map(|line| wrap(line, width))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}
/// Strip terminal control characters from untrusted provider/tool content.
pub fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}
pub fn wrap(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let limit = usize::from(width.max(1));
    let mut rows = vec![Line::default().style(line.style)];
    let mut column = 0;
    for span in line.spans {
        for cluster in safe_text(&span.content).graphemes(true) {
            let size = UnicodeWidthStr::width(cluster);
            if cluster == "\n" || column > 0 && column + size > limit {
                rows.push(Line::default().style(line.style));
                column = 0;
            }
            if cluster == "\n" {
                continue;
            }
            rows.last_mut()
                .expect("always a row")
                .spans
                .push(Span::styled(cluster.to_owned(), span.style));
            column += size;
        }
    }
    rows
}
