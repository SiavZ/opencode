//! Purpose: terminal application command routing and modal state, independent of service transport.
//! Scope: application lifetime.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/dialog/mod.rs
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/routes/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: modal input cannot submit a prompt; destructive operations require explicit confirmation.
//! Contract tests: ui-terminal/tests/application.rs

use crate::dialog::{Choice, Dialog, Dialogs, Kind, Outcome};
use crate::input::{UiEvent, UiKeyCode, UiKeyEventKind};
use crate::routes::{Navigation, Route};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Exit,
    Cancel,
    New,
    Sessions,
    Models,
    Variants,
    Agents,
    Timeline,
    Open(String),
    Model(String),
    Variant(String),
    Agent(String),
    Jump(String),
    Rename(String),
    Delete,
    Fork,
    ForkAt(String),
    Undo,
    Redo,
    Compact,
    Permission(String),
}

#[derive(Debug, Default)]
pub struct App {
    pub navigation: Navigation,
    pub dialogs: Dialogs,
    pub status: String,
    pub busy: bool,
    pub scroll: usize,
    pub thinking: bool,
    pub selected_model: Option<String>,
    pub selected_variant: Option<String>,
    pub selected_agent: Option<String>,
    pub pending_prompt: Option<String>,
    pub hidden_messages: Vec<String>,
    leader: bool,
}

impl App {
    pub fn session(&self) -> Option<&str> {
        match &self.navigation.current {
            Route::Session(id) => Some(id),
            _ => None,
        }
    }

    pub fn open(&mut self, id: String) {
        self.navigation.navigate(Route::Session(id));
        self.scroll = 0;
        self.dialogs.clear();
        self.hidden_messages.clear();
    }

    pub fn picker(&mut self, kind: Kind, title: &str, choices: Vec<Choice>) {
        self.dialogs.replace(Dialog::new(kind, title, choices));
    }

    pub fn error(&mut self, error: impl ToString) {
        self.status = error.to_string();
        self.picker(
            Kind::Alert,
            "Operation failed",
            vec![Choice::new("close", &self.status, "")],
        );
    }

    pub fn commands(&mut self) {
        let choices = [
            ("new", "New session", "Ctrl-X N"),
            ("sessions", "Switch session", "Ctrl-X L"),
            ("models", "Switch model", "Ctrl-X M"),
            ("variants", "Switch model variant", ""),
            ("agents", "Switch agent", "Ctrl-X A"),
            ("timeline", "Jump to message", ""),
            ("rename", "Rename session", ""),
            ("delete", "Delete session", "confirmation required"),
            ("fork", "Fork session", ""),
            ("undo", "Undo previous message", ""),
            ("redo", "Redo", ""),
            ("compact", "Compact session", ""),
            ("thinking", "Toggle thinking", ""),
            ("home", "Go home", ""),
            ("help", "Help", "F1"),
            ("exit", "Exit the app", "Ctrl-D"),
        ]
        .into_iter()
        .map(|(value, title, description)| Choice::new(value, title, description))
        .collect();
        self.picker(Kind::Commands, "Commands", choices);
    }

    pub fn command(&mut self, command: &str) -> Action {
        match command.trim_start_matches('/') {
            "new" => Action::New,
            "sessions" => Action::Sessions,
            "models" | "model" => Action::Models,
            "variants" | "variant" => Action::Variants,
            "agents" | "agent" => Action::Agents,
            "timeline" => Action::Timeline,
            "rename" => {
                self.picker(Kind::Rename, "Rename session", vec![]);
                Action::None
            }
            "delete" => {
                self.picker(
                    Kind::Delete,
                    "Delete session permanently?",
                    vec![
                        Choice::new("cancel", "Cancel", ""),
                        Choice::new("delete", "Delete", "cannot be undone"),
                    ],
                );
                Action::None
            }
            "fork" => Action::Fork,
            "undo" => Action::Undo,
            "redo" => Action::Redo,
            "compact" => Action::Compact,
            "thinking" => {
                self.thinking = !self.thinking;
                Action::None
            }
            "home" => {
                self.navigation.navigate(Route::Home);
                self.scroll = 0;
                Action::None
            }
            "help" => {
                self.picker(
                    Kind::Alert,
                    "Help",
                    vec![Choice::new(
                        "close",
                        "Ctrl-P commands | Ctrl-X prefix | PageUp/PageDown scroll",
                        "Enter send; Shift-Enter newline; Esc cancel; Ctrl-D quit",
                    )],
                );
                Action::None
            }
            "exit" | "quit" => Action::Exit,
            _ => {
                self.error(format!("Unknown command: {command}"));
                Action::None
            }
        }
    }

    /// Returns Some whenever the application consumed the event, including modal no-ops.
    pub fn handle(&mut self, event: &UiEvent) -> Option<Action> {
        if self.dialogs.top().is_some() {
            return Some(match self.dialogs.handle(event) {
                Outcome::Select(kind, value) => match kind {
                    Kind::Commands => self.command(&value),
                    Kind::Sessions => Action::Open(value),
                    Kind::Models => Action::Model(value),
                    Kind::Variants => Action::Variant(value),
                    Kind::Agents => Action::Agent(value),
                    Kind::Timeline => Action::Jump(value),
                    Kind::Fork => Action::ForkAt(value),
                    Kind::Rename => Action::Rename(value),
                    Kind::Delete if value == "delete" => Action::Delete,
                    Kind::Permission | Kind::PermissionConfirm => Action::Permission(value),
                    _ => Action::None,
                },
                _ => Action::None,
            });
        }
        let UiEvent::Key(key) = event else {
            return None;
        };
        if key.kind == UiKeyEventKind::Release {
            return Some(Action::None);
        }
        if self.leader {
            self.leader = false;
            return Some(match key.code {
                UiKeyCode::Char('n') => Action::New,
                UiKeyCode::Char('l') => Action::Sessions,
                UiKeyCode::Char('m') => Action::Models,
                UiKeyCode::Char('a') => Action::Agents,
                UiKeyCode::Char('q') => Action::Exit,
                _ => Action::None,
            });
        }
        match key.code {
            UiKeyCode::Char('x') if key.modifiers.control => {
                self.leader = true;
                Some(Action::None)
            }
            UiKeyCode::Char('p') if key.modifiers.control => {
                self.commands();
                Some(Action::None)
            }
            UiKeyCode::F(1) => Some(self.command("help")),
            UiKeyCode::Char('d') if key.modifiers.control => Some(Action::Exit),
            UiKeyCode::Char('c') if key.modifiers.control => Some(if self.busy {
                Action::Cancel
            } else {
                Action::Exit
            }),
            UiKeyCode::Esc if self.busy => Some(Action::Cancel),
            UiKeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(10);
                Some(Action::None)
            }
            UiKeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
                Some(Action::None)
            }
            _ => None,
        }
    }
}
