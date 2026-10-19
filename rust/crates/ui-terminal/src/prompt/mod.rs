//! Purpose: own grapheme-safe prompt edits, selection, undo and cursor movement.
//! Scope: per prompt widget, with no filesystem or terminal effects.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/input.rs
//! |-- ui-terminal/src/prompt/layout.rs
//! `-- ui-terminal/src/prompt/keys.rs
//! External:
//! `-- unicode_segmentation::UnicodeSegmentation
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
//! Invariants: cursor and selection always lie on extended grapheme boundaries.
//! Contract tests: ui-terminal/tests/editor.rs

mod keys;
mod layout;
pub use layout::{EditorLayout, display_slice, mention_trigger_index, offset_width};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Start,
    Finish,
    WordLeft,
    WordRight,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    None,
    Changed,
    Submit(String),
}
#[derive(Debug, Clone, Default)]
pub struct Editor {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    preferred_column: Option<usize>,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    history: Vec<String>,
    history_index: usize,
}
impl Editor {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor
            .filter(|anchor| *anchor != self.cursor)
            .map(|anchor| anchor.min(self.cursor)..anchor.max(self.cursor))
    }
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
    }
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }
    fn checkpoint(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    pub fn insert(&mut self, text: &str) {
        self.checkpoint();
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        // Inserting combining text can merge with either adjacent cluster.
        self.cursor = self.boundary_after(range.start + text.len());
        self.anchor = None;
        self.preferred_column = None;
    }
    pub fn paste(&mut self, text: &str) {
        self.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
    }
    pub fn delete(&mut self, backwards: bool) {
        let range = self.selection().unwrap_or_else(|| {
            if backwards {
                self.previous(self.cursor)..self.cursor
            } else {
                self.cursor..self.next(self.cursor)
            }
        });
        if range.is_empty() {
            return;
        }
        self.checkpoint();
        self.text.replace_range(range.clone(), "");
        self.cursor = self.boundary_after(range.start);
        self.anchor = None;
        self.preferred_column = None;
    }
    pub fn undo(&mut self) {
        if let Some(state) = self.undo.pop() {
            self.redo.push((self.text.clone(), self.cursor));
            (self.text, self.cursor) = state;
            self.anchor = None;
            self.preferred_column = None;
        }
    }
    pub fn redo(&mut self) {
        if let Some(state) = self.redo.pop() {
            self.undo.push((self.text.clone(), self.cursor));
            (self.text, self.cursor) = state;
            self.anchor = None;
            self.preferred_column = None;
        }
    }
    pub fn move_cursor(&mut self, movement: Movement, selecting: bool) {
        if selecting {
            self.anchor.get_or_insert(self.cursor);
        }
        if !selecting {
            if let Some(range) = self.selection() {
                if matches!(movement, Movement::Left | Movement::Right) {
                    self.cursor = if movement == Movement::Left {
                        range.start
                    } else {
                        range.end
                    };
                    self.anchor = None;
                    self.preferred_column = None;
                    return;
                }
            }
            self.anchor = None;
        }
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        self.cursor = match movement {
            Movement::Left => self.previous(self.cursor),
            Movement::Right => self.next(self.cursor),
            Movement::Start => 0,
            Movement::Finish => self.text.len(),
            Movement::Home => start,
            Movement::End => end,
            Movement::WordLeft => self.word_left(),
            Movement::WordRight => self.word_right(),
            Movement::Up | Movement::Down => {
                let column = self
                    .preferred_column
                    .unwrap_or_else(|| layout::width(&self.text[start..self.cursor]));
                self.preferred_column = Some(column);
                let target = if movement == Movement::Up {
                    if start == 0 {
                        return;
                    }
                    let previous = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
                    previous..start - 1
                } else {
                    if end == self.text.len() {
                        return;
                    }
                    let next = self.text[end + 1..]
                        .find('\n')
                        .map_or(self.text.len(), |i| end + 1 + i);
                    end + 1..next
                };
                target.start + layout::column_index(&self.text[target.clone()], column)
            }
        };
        if !matches!(movement, Movement::Up | Movement::Down) {
            self.preferred_column = None;
        }
    }
    fn previous(&self, position: usize) -> usize {
        self.text[..position]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }
    fn next(&self, position: usize) -> usize {
        self.text[position..]
            .graphemes(true)
            .next()
            .map_or(position, |g| position + g.len())
    }
    fn boundary_after(&self, position: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([self.text.len()])
            .find(|i| *i >= position)
            .unwrap_or(self.text.len())
    }
    fn word_left(&self) -> usize {
        let mut position = self.cursor;
        while position > 0
            && self.text[self.previous(position)..position]
                .chars()
                .all(char::is_whitespace)
        {
            position = self.previous(position);
        }
        while position > 0
            && !self.text[self.previous(position)..position]
                .chars()
                .all(char::is_whitespace)
        {
            position = self.previous(position);
        }
        position
    }
    fn word_right(&self) -> usize {
        let mut position = self.cursor;
        while position < self.text.len()
            && self.text[position..self.next(position)]
                .chars()
                .all(char::is_whitespace)
        {
            position = self.next(position);
        }
        while position < self.text.len()
            && !self.text[position..self.next(position)]
                .chars()
                .all(char::is_whitespace)
        {
            position = self.next(position);
        }
        position
    }
    pub fn remember(&mut self, text: String) {
        if self.history.last() != Some(&text) {
            self.history.push(text);
        }
        if self.history.len() > 50 {
            self.history.remove(0);
        }
        self.history_index = self.history.len();
    }
    pub fn history_move(&mut self, backwards: bool) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let current = self.history.get(self.history_index);
        if !self.text.is_empty() && current != Some(&self.text) {
            return false;
        }
        let index = if backwards {
            self.history_index.saturating_sub(1)
        } else {
            (self.history_index + 1).min(self.history.len())
        };
        self.set_text(self.history.get(index).cloned().unwrap_or_default());
        self.history_index = index;
        if backwards {
            self.cursor = 0;
        }
        true
    }
}
