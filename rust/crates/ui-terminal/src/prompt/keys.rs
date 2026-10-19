//! Purpose: dispatch normalized keys and bracketed paste into prompt editing actions.
//! Scope: one input event.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/prompt/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/prompt/mod.rs
//! Invariants: releases never edit; paste never submits.
//! Contract tests: ui-terminal/tests/editor.rs
use super::{Editor, EditorAction, Movement};
use crate::input::{UiEvent, UiKeyCode, UiKeyEventKind};

impl Editor {
    pub fn handle(&mut self, event: &UiEvent) -> EditorAction {
        if let UiEvent::Paste(text) = event {
            self.paste(text);
            return EditorAction::Changed;
        }
        let UiEvent::Key(key) = event else {
            return EditorAction::None;
        };
        if key.kind == UiKeyEventKind::Release {
            return EditorAction::None;
        }
        let control = key.modifiers.control;
        let alt = key.modifiers.alt;
        let shift = key.modifiers.shift;
        let movement = match key.code {
            UiKeyCode::Left if control || alt => Some(Movement::WordLeft),
            UiKeyCode::Right if control || alt => Some(Movement::WordRight),
            UiKeyCode::Left => Some(Movement::Left),
            UiKeyCode::Right => Some(Movement::Right),
            UiKeyCode::Home if control => Some(Movement::Start),
            UiKeyCode::End if control => Some(Movement::Finish),
            UiKeyCode::Home => Some(Movement::Home),
            UiKeyCode::End => Some(Movement::End),
            UiKeyCode::Char('a') if control => Some(Movement::Home),
            UiKeyCode::Char('e') if control => Some(Movement::End),
            UiKeyCode::Char('b') if control => Some(Movement::Left),
            UiKeyCode::Char('f') if control => Some(Movement::Right),
            UiKeyCode::Up => Some(Movement::Up),
            UiKeyCode::Down => Some(Movement::Down),
            _ => None,
        };
        if let Some(movement) = movement {
            if !shift && key.code == UiKeyCode::Up && self.cursor == 0 {
                self.history_move(true);
            } else if !shift && key.code == UiKeyCode::Down && self.cursor == self.text.len() {
                self.history_move(false);
            } else {
                self.move_cursor(movement, shift);
            }
            return EditorAction::Changed;
        }
        match key.code {
            UiKeyCode::Enter if shift || alt || control => self.insert("\n"),
            UiKeyCode::Enter => {
                if self.text.trim().is_empty() {
                    return EditorAction::None;
                }
                let text = self.text.clone();
                self.remember(text.clone());
                self.set_text("");
                self.undo.clear();
                self.redo.clear();
                return EditorAction::Submit(text);
            }
            UiKeyCode::Backspace | UiKeyCode::Char('h')
                if key.code == UiKeyCode::Backspace || control =>
            {
                self.delete(true)
            }
            UiKeyCode::Delete => self.delete(false),
            UiKeyCode::Char('z') if control && shift => self.redo(),
            UiKeyCode::Char('z') if control => self.undo(),
            UiKeyCode::Char('y') if control => self.redo(),
            UiKeyCode::Char('w') if control => {
                self.move_cursor(Movement::WordLeft, true);
                self.delete(true);
            }
            UiKeyCode::Char('u') if control => {
                self.move_cursor(Movement::Home, true);
                self.delete(true);
            }
            UiKeyCode::Char('k') if control => {
                self.move_cursor(Movement::End, true);
                self.delete(false);
            }
            UiKeyCode::Char(c)
                if !control && !alt && !key.modifiers.super_key && !c.is_control() =>
            {
                self.insert(&c.to_string())
            }
            _ => return EditorAction::None,
        }
        EditorAction::Changed
    }
}
