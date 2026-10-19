//! Purpose: parse streamed CommonMark text into styled terminal spans.
//! Scope: pure rendering of the current complete prefix, including unclosed fences.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! |-- pulldown_cmark::{Event, Options, Parser, Tag, TagEnd}
//! `-- ratatui::{style, text}
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! Invariants: raw HTML and links never execute terminal control sequences.
//! Contract tests: ui-terminal/tests/rendering.rs
use super::safe_text;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    let mut styles = vec![Style::default()];
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote = 0;
    for event in Parser::new_ext(
        text,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        let current = *styles.last().expect("base style");
        match event {
            Event::Start(tag) => {
                let style = match &tag {
                    Tag::Strong | Tag::Heading { .. } => current.add_modifier(Modifier::BOLD),
                    Tag::Emphasis => current.add_modifier(Modifier::ITALIC),
                    Tag::Strikethrough => current.add_modifier(Modifier::CROSSED_OUT),
                    Tag::CodeBlock(_) => current.fg(Color::Yellow),
                    Tag::Link { .. } => current.fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
                    _ => current,
                };
                match tag {
                    Tag::List(start) => lists.push(start),
                    Tag::BlockQuote(_) => {
                        quote += 1;
                        push(&mut lines, "│ ", current.fg(Color::DarkGray));
                    }
                    Tag::Item => {
                        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                            lines.push(Line::default());
                        }
                        let prefix = lists.last_mut().map_or_else(
                            || "• ".to_owned(),
                            |item| {
                                if let Some(index) = item {
                                    let prefix = format!("{index}. ");
                                    *index += 1;
                                    prefix
                                } else {
                                    "• ".to_owned()
                                }
                            },
                        );
                        push(
                            &mut lines,
                            &format!("{}{prefix}", "  ".repeat(lists.len().saturating_sub(1))),
                            current,
                        );
                    }
                    Tag::TableCell => {
                        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                            push(&mut lines, " │ ", current);
                        }
                    }
                    _ => {}
                }
                styles.push(style);
            }
            Event::End(tag) => {
                styles.pop();
                match tag {
                    TagEnd::List(_) => {
                        lists.pop();
                    }
                    TagEnd::BlockQuote(_) => {
                        quote -= 1;
                    }
                    TagEnd::Paragraph
                    | TagEnd::Heading(_)
                    | TagEnd::CodeBlock
                    | TagEnd::Item
                    | TagEnd::TableRow
                    | TagEnd::TableHead => {
                        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                            lines.push(Line::default());
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => {
                push(&mut lines, &value, current)
            }
            Event::Code(value) => push(&mut lines, &value, current.fg(Color::Yellow)),
            Event::SoftBreak | Event::HardBreak => {
                lines.push(Line::default());
                if quote > 0 {
                    push(&mut lines, &"│ ".repeat(quote), current.fg(Color::DarkGray));
                }
            }
            Event::Rule => {
                push(&mut lines, "────────", current.fg(Color::DarkGray));
                lines.push(Line::default());
            }
            Event::TaskListMarker(checked) => {
                push(&mut lines, if checked { "[x] " } else { "[ ] " }, current)
            }
            Event::FootnoteReference(value) => push(&mut lines, &format!("[{value}]"), current),
            _ => {}
        }
    }
    if lines.len() > 1 && lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    lines
}
fn push(lines: &mut Vec<Line<'static>>, text: &str, style: Style) {
    for (index, part) in safe_text(text).split('\n').enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        if !part.is_empty() {
            lines
                .last_mut()
                .expect("base row")
                .spans
                .push(Span::styled(part.to_owned(), style));
        }
    }
}
