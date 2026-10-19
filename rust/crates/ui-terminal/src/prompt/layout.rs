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
//! Invariants: wrapping never splits a grapheme; hard newlines do not double-wrap.
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
    positions: Vec<(usize, (u16, u16))>,
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
        let mut positions = Vec::new();
        let mut x = 0;
        let mut cursor = (0, 0);
        let selection = self.selection();
        for (index, cluster) in self.text.grapheme_indices(true) {
            let size = cluster_width(cluster);
            if cluster != "\n" && x > 0 && x + size > limit {
                lines.push(Line::default());
                x = 0;
            }
            let point = (
                x.min(limit - 1) as u16,
                (lines.len() - 1).min(u16::MAX as usize) as u16,
            );
            positions.push((index, point));
            if index == self.cursor {
                cursor = point;
            }
            if cluster == "\n" {
                lines.push(Line::default());
                x = 0;
                continue;
            }
            let selected = selection
                .as_ref()
                .is_some_and(|range| range.contains(&index));
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
            lines
                .last_mut()
                .expect("layout always has a row")
                .spans
                .push(Span::styled(content, style));
            x += size;
        }
        // Keep a real cell available for the insertion caret at an exact right margin.
        if x >= limit {
            lines.push(Line::default());
            x = 0;
        }
        let end = (x as u16, (lines.len() - 1).min(u16::MAX as usize) as u16);
        positions.push((self.text.len(), end));
        if self.cursor == self.text.len() {
            cursor = end;
        }
        EditorLayout {
            lines,
            cursor,
            positions,
        }
    }
    pub(super) fn move_visual(&mut self, direction: i32, selecting: bool) {
        let layout = self.layout(self.viewport_width.unwrap_or(u16::MAX));
        let column = self
            .preferred_column
            .unwrap_or(usize::from(layout.cursor.0));
        let row = i32::from(layout.cursor.1) + direction;
        let target = layout
            .positions
            .iter()
            .rfind(|(_, point)| i32::from(point.1) == row && usize::from(point.0) <= column)
            .map(|(offset, _)| *offset)
            .or_else(|| {
                layout
                    .positions
                    .iter()
                    .find(|(_, point)| i32::from(point.1) == row)
                    .map(|(offset, _)| *offset)
            })
            .unwrap_or(if direction < 0 { 0 } else { self.text.len() });
        if selecting {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        self.preferred_column = Some(column);
    }
    pub(super) fn visual_edge(&mut self, end: bool, selecting: bool) {
        let layout = self.layout(self.viewport_width.unwrap_or(u16::MAX));
        let mut positions = layout
            .positions
            .iter()
            .filter(|(_, point)| point.1 == layout.cursor.1);
        let target = if end {
            positions.next_back()
        } else {
            positions.next()
        };
        if selecting {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        if let Some((offset, _)) = target {
            self.cursor = *offset;
        }
        self.preferred_column = None;
    }
}
/// Display offsets count newlines as one, unlike terminal column width.
pub fn offset_width(value: &str) -> usize {
    value
        .graphemes(true)
        .map(|g| {
            if g == "\n" {
                1
            } else if g.chars().all(char::is_control) {
                0
            } else {
                UnicodeWidthStr::width(g)
            }
        })
        .sum()
}
pub fn display_slice(value: &str, start: usize, end: usize) -> &str {
    &value[display_index(value, start)..display_index(value, end.max(start))]
}
fn display_index(value: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
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
