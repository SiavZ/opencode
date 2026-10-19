//! Purpose: searchable selection, text entry and confirmation dialogs with modal input isolation.
//! Scope: application lifetime.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/input.rs
//! External:
//! `-- unicode_segmentation::UnicodeSegmentation
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/app.rs
//! Invariants: disabled choices cannot be selected; only the top dialog receives keys.
//! Contract tests: ui-terminal/tests/application.rs

use crate::input::{UiEvent, UiKeyCode, UiKeyEventKind};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub title: String,
    pub description: String,
    pub disabled: bool,
}

impl Choice {
    pub fn new(
        value: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            value: value.into(),
            title: title.into(),
            description: description.into(),
            disabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Commands,
    Sessions,
    Models,
    Agents,
    Timeline,
    Rename,
    Delete,
    Permission,
    Alert,
}

#[derive(Debug)]
pub struct Dialog {
    pub kind: Kind,
    pub title: String,
    pub query: String,
    pub choices: Vec<Choice>,
    selected: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
    Select(Kind, String),
}

impl Dialog {
    pub fn new(kind: Kind, title: impl Into<String>, choices: Vec<Choice>) -> Self {
        Self {
            kind,
            title: title.into(),
            query: String::new(),
            choices,
            selected: 0,
        }
    }

    pub fn filtered(&self) -> Vec<&Choice> {
        let query = self.query.to_lowercase();
        self.choices
            .iter()
            .filter(|choice| {
                !choice.disabled
                    && (query.is_empty()
                        || format!("{} {}", choice.title, choice.description)
                            .to_lowercase()
                            .contains(&query))
            })
            .collect()
    }

    pub fn handle(&mut self, event: &UiEvent) -> Outcome {
        let UiEvent::Key(key) = event else {
            if let UiEvent::Paste(text) = event {
                self.query
                    .extend(text.chars().filter(|ch| !ch.is_control()));
                self.selected = 0;
            }
            return Outcome::None;
        };
        if key.kind == UiKeyEventKind::Release {
            return Outcome::None;
        }
        if key.code == UiKeyCode::Esc || key.code == UiKeyCode::Char('c') && key.modifiers.control {
            return Outcome::Close;
        }
        match key.code {
            UiKeyCode::Enter if self.kind == Kind::Rename => {
                return Outcome::Select(self.kind, self.query.clone());
            }
            UiKeyCode::Enter => {
                if let Some(choice) = self.filtered().get(self.selected) {
                    return Outcome::Select(self.kind, choice.value.clone());
                }
            }
            UiKeyCode::Up | UiKeyCode::BackTab => self.step(-1),
            UiKeyCode::Down | UiKeyCode::Tab => self.step(1),
            UiKeyCode::PageUp => self.step(-10),
            UiKeyCode::PageDown => self.step(10),
            UiKeyCode::Home => self.selected = 0,
            UiKeyCode::End => self.selected = self.filtered().len().saturating_sub(1),
            UiKeyCode::Backspace => {
                if let Some((index, _)) = self.query.grapheme_indices(true).next_back() {
                    self.query.truncate(index);
                }
                self.selected = 0;
            }
            UiKeyCode::Char(ch) if !key.modifiers.control && !key.modifiers.alt => {
                if !ch.is_control() {
                    self.query.push(ch);
                    self.selected = 0;
                }
            }
            _ => {}
        }
        Outcome::None
    }

    fn step(&mut self, delta: isize) {
        let count = self.filtered().len();
        if count != 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        }
    }

    pub fn lines(&self, rows: usize) -> Vec<String> {
        let mut lines = vec![self.title.clone(), format!("Search: {}", self.query)];
        if self.kind == Kind::Rename {
            lines[1] = format!("Title: {}", self.query);
        }
        let filtered = self.filtered();
        let start = self.selected.saturating_sub(rows.saturating_sub(1));
        lines.extend(
            filtered
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(index, choice)| {
                    format!(
                        "{} {}  {}",
                        if index == self.selected { ">" } else { " " },
                        choice.title,
                        choice.description
                    )
                }),
        );
        if filtered.is_empty() && self.kind != Kind::Rename {
            lines.push("No results found".into());
        }
        lines.push("Enter selects | Esc closes".into());
        lines
    }
}

#[derive(Debug, Default)]
pub struct Dialogs(Vec<Dialog>);

impl Dialogs {
    pub fn top(&self) -> Option<&Dialog> {
        self.0.last()
    }
    pub fn replace(&mut self, dialog: Dialog) {
        self.0.clear();
        self.0.push(dialog);
    }
    pub fn push(&mut self, dialog: Dialog) {
        self.0.push(dialog);
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn handle(&mut self, event: &UiEvent) -> Outcome {
        let result = self
            .0
            .last_mut()
            .map(|dialog| dialog.handle(event))
            .unwrap_or(Outcome::None);
        if matches!(result, Outcome::Close | Outcome::Select(..)) {
            self.0.pop();
        }
        result
    }
}
