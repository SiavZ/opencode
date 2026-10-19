//! Purpose: render real line changes with old/new numbers and missing-newline markers.
//! Scope: pure unified inline diff projection.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! |-- ratatui::{style, text}
//! `-- similar::{ChangeTag, TextDiff}
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/timeline/mod.rs
//! Invariants: equal, inserted and deleted lines retain source order and newline state.
//! Contract tests: ui-terminal/tests/rendering.rs
use super::safe_text;
use ratatui::{
    style::{Color, Style},
    text::Line,
};
use similar::{ChangeTag, TextDiff};

pub fn render(before: &str, after: &str) -> Vec<Line<'static>> {
    TextDiff::from_lines(before, after)
        .iter_all_changes()
        .flat_map(|change| {
            let (sign, color) = match change.tag() {
                ChangeTag::Delete => ('-', Color::Red),
                ChangeTag::Insert => ('+', Color::Green),
                ChangeTag::Equal => (' ', Color::DarkGray),
            };
            let old = change
                .old_index()
                .map_or_else(String::new, |i| (i + 1).to_string());
            let new = change
                .new_index()
                .map_or_else(String::new, |i| (i + 1).to_string());
            let line = format!(
                "{old:>4} {new:>4} {sign} {}",
                safe_text(change.value().trim_end_matches('\n').trim_end_matches('\r'))
            );
            let mut lines = vec![Line::styled(line, Style::default().fg(color))];
            if change.missing_newline() {
                lines.push(Line::styled(
                    "\\ No newline at end of file",
                    Style::default().fg(Color::Yellow),
                ));
            }
            lines
        })
        .collect()
}

/// Preserve a tool's existing unified patch, including hunk and file headers.
pub fn render_patch(patch: &str) -> Vec<Line<'static>> {
    patch
        .lines()
        .map(|line| {
            let color = match line.as_bytes().first() {
                Some(b'+') if !line.starts_with("+++") => Color::Green,
                Some(b'-') if !line.starts_with("---") => Color::Red,
                Some(b'@') => Color::Cyan,
                _ => Color::DarkGray,
            };
            Line::styled(safe_text(line), Style::default().fg(color))
        })
        .collect()
}
