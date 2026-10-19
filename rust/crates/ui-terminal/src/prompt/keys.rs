//! Purpose: dispatch baseline prompt commands and default keys into editor operations.
//! Scope: one input event; app commands and clipboard IO stay with the caller.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/prompt/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/prompt/mod.rs
//! Invariants: releases never edit; paste never submits; Ctrl-Z remains terminal suspend.
//! Contract tests: ui-terminal/tests/editor.rs
use super::{Editor, EditorAction, Movement};
use crate::input::{UiEvent, UiKeyCode, UiKeyEventKind};

impl Editor {
    /// Accept a command resolved by the app's configurable keybinding engine.
    pub fn command(&mut self, command: &str) -> EditorAction {
        let movement = match command {
            "input.move.left" | "input.select.left" => Some(Movement::Left),
            "input.move.right" | "input.select.right" => Some(Movement::Right),
            "input.line.home" | "input.select.line.home" => Some(Movement::Home),
            "input.line.end" | "input.select.line.end" => Some(Movement::End),
            "input.buffer.home" | "input.select.buffer.home" => Some(Movement::Start),
            "input.buffer.end" | "input.select.buffer.end" => Some(Movement::Finish),
            "input.word.backward" | "input.select.word.backward" => Some(Movement::WordLeft),
            "input.word.forward" | "input.select.word.forward" => Some(Movement::WordRight),
            _ => None,
        };
        if let Some(movement) = movement {
            self.move_cursor(movement, command.starts_with("input.select."));
            return EditorAction::Changed;
        }
        match command {
            "input.move.up" | "input.select.up" => {
                self.move_visual(-1, command == "input.select.up")
            }
            "input.move.down" | "input.select.down" => {
                self.move_visual(1, command == "input.select.down")
            }
            "input.visual.line.home" | "input.select.visual.line.home" => {
                self.visual_edge(false, command.starts_with("input.select."))
            }
            "input.visual.line.end" | "input.select.visual.line.end" => {
                self.visual_edge(true, command.starts_with("input.select."))
            }
            "input.select.all" => self.select_all(),
            "input.newline" => self.insert("\n"),
            "input.backspace" => self.delete(true),
            "input.delete" => self.delete(false),
            "input.undo" => self.undo(),
            "input.redo" => self.redo(),
            "input.delete.word.backward" => {
                self.move_cursor(Movement::WordLeft, true);
                self.delete(true);
            }
            "input.delete.word.forward" => {
                self.move_cursor(Movement::WordRight, true);
                self.delete(false);
            }
            "input.delete.to.line.start" => {
                self.move_cursor(Movement::Home, true);
                self.delete(true);
            }
            "input.delete.to.line.end" => {
                self.move_cursor(Movement::End, true);
                self.delete(false);
            }
            "input.delete.line" => {
                self.move_cursor(Movement::Home, false);
                self.move_cursor(Movement::End, true);
                if self.cursor < self.text.len() {
                    self.move_cursor(Movement::Right, true);
                }
                self.delete(false);
            }
            "prompt.history.previous" => {
                self.history_move(true);
            }
            "prompt.history.next" => {
                self.history_move(false);
            }
            "input.submit" | "prompt.submit" => {
                if self.text.is_empty() {
                    return EditorAction::None;
                }
                let text = self.text.clone();
                self.remember(text.clone());
                self.set_text("");
                self.undo.clear();
                self.redo.clear();
                return EditorAction::Submit(text);
            }
            _ => return EditorAction::None,
        }
        EditorAction::Changed
    }
    pub fn handle_at(&mut self, event: &UiEvent, width: u16) -> EditorAction {
        self.viewport_width = Some(width.max(1));
        self.handle(event)
    }
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
        let super_key = key.modifiers.super_key;
        let command = match key.code {
            UiKeyCode::Left if control || alt => "input.word.backward",
            UiKeyCode::Right if control || alt => "input.word.forward",
            UiKeyCode::Left => "input.move.left",
            UiKeyCode::Right => "input.move.right",
            UiKeyCode::Home => "input.buffer.home",
            UiKeyCode::End => "input.buffer.end",
            UiKeyCode::Char('a') if super_key => "input.select.all",
            UiKeyCode::Char('a') if control => "input.line.home",
            UiKeyCode::Char('e') if control => "input.line.end",
            UiKeyCode::Char('a') if alt => "input.visual.line.home",
            UiKeyCode::Char('e') if alt => "input.visual.line.end",
            UiKeyCode::Char('b') if alt => "input.word.backward",
            UiKeyCode::Char('f') if alt => "input.word.forward",
            UiKeyCode::Char('b') if control => "input.move.left",
            UiKeyCode::Char('f') if control => "input.move.right",
            UiKeyCode::Up if !shift && self.cursor == 0 => "prompt.history.previous",
            UiKeyCode::Down if !shift && self.cursor == self.text.len() => "prompt.history.next",
            UiKeyCode::Up => "input.move.up",
            UiKeyCode::Down => "input.move.down",
            UiKeyCode::Enter if shift || alt || control => "input.newline",
            UiKeyCode::Char('j') if control => "input.newline",
            UiKeyCode::Enter => "input.submit",
            UiKeyCode::Backspace if control || alt => "input.delete.word.backward",
            UiKeyCode::Backspace => "input.backspace",
            UiKeyCode::Delete if control || alt => "input.delete.word.forward",
            UiKeyCode::Delete => "input.delete",
            UiKeyCode::Char('d') if control && shift => "input.delete.line",
            UiKeyCode::Char('d') if control => "input.delete",
            UiKeyCode::Char('d') if alt => "input.delete.word.forward",
            UiKeyCode::Char('z') if super_key && shift => "input.redo",
            UiKeyCode::Char('z') if super_key => "input.undo",
            UiKeyCode::Char('-') if control => "input.undo",
            UiKeyCode::Char('.') if control => "input.redo",
            UiKeyCode::Char('w') if control => "input.delete.word.backward",
            UiKeyCode::Char('u') if control => "input.delete.to.line.start",
            UiKeyCode::Char('k') if control => "input.delete.to.line.end",
            UiKeyCode::Char(c) if !control && !alt && !super_key && !c.is_control() => {
                self.insert(&c.to_string());
                return EditorAction::Changed;
            }
            _ => return EditorAction::None,
        };
        let selection = shift.then_some(match command {
            "input.move.left" => "input.select.left",
            "input.move.right" => "input.select.right",
            "input.move.up" => "input.select.up",
            "input.move.down" => "input.select.down",
            "input.line.home" => "input.select.line.home",
            "input.line.end" => "input.select.line.end",
            "input.buffer.home" => "input.select.buffer.home",
            "input.buffer.end" => "input.select.buffer.end",
            "input.visual.line.home" => "input.select.visual.line.home",
            "input.visual.line.end" => "input.select.visual.line.end",
            "input.word.backward" => "input.select.word.backward",
            "input.word.forward" => "input.select.word.forward",
            other => other,
        });
        self.command(selection.unwrap_or(command))
    }
}
