//! Purpose: lay out prompt clusters and selection using terminal display cells.
//! Scope: pure layout at the supplied viewport width.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/prompt/mod.rs
//! External:
//! |-- ratatui::{style, text}
//! |-- unicode_segmentation::UnicodeSegmentation
//! `-- unicode_width::UnicodeWidthStr
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/prompt/mod.rs
//! Invariants: wrapping does not split a grapheme; newline occupies its own offset.
//! Contract tests: ui-terminal/tests/editor.rs
use super::Editor;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct EditorLayout {
    pub lines: Vec<Line<'static>>,
    pub cursor: (u16, u16),
}
pub(super) fn width(value: &str) -> usize {
    value.graphemes(true).map(cluster_width).sum()
}
fn cluster_width(value: &str) -> usize {
    if value == "\t" {
        4
    } else {
        UnicodeWidthStr::width(value)
    }
}
pub(super) fn column_index(value: &str, column: usize) -> usize {
    let mut width = 0;
    for (index, cluster) in value.grapheme_indices(true) {
        if width + cluster_width(cluster) > column {
            return index;
        }
        width += cluster_width(cluster);
    }
    value.len()
}
impl Editor {
    pub fn layout(&self, width: u16) -> EditorLayout {
        let limit = usize::from(width.max(1));
        let mut lines = vec![Line::default()];
        let mut x = 0;
        let mut cursor = (0, 0);
        for (index, cluster) in self.text.grapheme_indices(true) {
            let size = cluster_width(cluster);
            if cluster != "\n" && x > 0 && x + size > limit {
                lines.push(Line::default());
                x = 0;
            }
            if index == self.cursor {
                cursor = (x as u16, (lines.len() - 1).min(u16::MAX as usize) as u16);
            }
            if cluster == "\n" {
                lines.push(Line::default());
                x = 0;
                continue;
            }
            let selected = self.selection().is_some_and(|range| range.contains(&index));
            let content = if cluster == "\t" {
                "    ".to_owned()
            } else {
                cluster.chars().filter(|c| !c.is_control()).collect()
            };
            let style = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            // A cluster wider than the entire viewport is clipped by the terminal widget.
            lines
                .last_mut()
                .expect("layout always has a row")
                .spans
                .push(Span::styled(content, style));
            x += size;
            if x >= limit {
                lines.push(Line::default());
                x = 0;
            }
        }
        if self.cursor == self.text.len() {
            cursor = (x as u16, (lines.len() - 1).min(u16::MAX as usize) as u16);
        }
        EditorLayout { lines, cursor }
    }
}
/// Display offsets count newlines as one, unlike terminal column width.
pub fn offset_width(value: &str) -> usize {
    value
        .graphemes(true)
        .map(|g| if g == "\n" { 1 } else { cluster_width(g) })
        .sum()
}
pub fn display_slice(value: &str, start: usize, end: usize) -> &str {
    &value[display_index(value, start)..display_index(value, end.max(start))]
}
fn display_index(value: &str, offset: usize) -> usize {
    let mut width = 0;
    for (index, cluster) in value.grapheme_indices(true) {
        width += offset_width(cluster);
        if width > offset {
            return index;
        }
    }
    value.len()
}
pub fn mention_trigger_index(value: &str, offset: usize) -> Option<usize> {
    let text = display_slice(value, 0, offset);
    let index = text.rfind('@')?;
    if text[..index]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
        || text[index..].chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(offset_width(&text[..index]))
}
