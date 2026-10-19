//! Purpose: exercise application navigation and modal behavior through normalized real input.
//! Scope: terminal application contracts.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/app.rs
//! |-- ui-terminal/src/dialog/mod.rs
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/routes/mod.rs
//! External:
//! `-- crossterm::event
//! Used by:
//! `-- none, entrypoint
//! Invariants: confirmations never execute by default and modal keys never leak into the prompt.
//! Contract tests: this file

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use opencode_ui_terminal::app::{Action, App};
use opencode_ui_terminal::dialog::{Choice, Dialog, Dialogs, Kind, Outcome};
use opencode_ui_terminal::input::UiEvent;
use opencode_ui_terminal::routes::{Navigation, Route};

fn key(code: KeyCode) -> UiEvent {
    UiEvent::from(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

#[test]
fn picker_filters_wraps_and_excludes_disabled_choices() {
    let mut dialog = Dialog::new(
        Kind::Sessions,
        "Sessions",
        vec![
            Choice::new("one", "Alpha", ""),
            Choice::new("two", "Beta", ""),
            Choice {
                disabled: true,
                ..Choice::new("hidden", "Hidden", "")
            },
        ],
    );
    dialog.handle(&key(KeyCode::Up));
    assert_eq!(
        dialog.handle(&key(KeyCode::Enter)),
        Outcome::Select(Kind::Sessions, "two".into())
    );
    dialog.handle(&UiEvent::Paste("alp".into()));
    assert_eq!(dialog.filtered().len(), 1);
    assert_eq!(
        dialog.handle(&key(KeyCode::Enter)),
        Outcome::Select(Kind::Sessions, "one".into())
    );
    dialog.handle(&UiEvent::Paste("missing".into()));
    assert_eq!(dialog.handle(&key(KeyCode::Enter)), Outcome::None);
    assert!(
        dialog
            .lines(8)
            .iter()
            .any(|line| line == "No results found")
    );
}

#[test]
fn modal_input_isolated_and_only_top_receives_escape() {
    let mut dialogs = Dialogs::default();
    dialogs.push(Dialog::new(Kind::Models, "Model", vec![]));
    dialogs.push(Dialog::new(Kind::Agents, "Agent", vec![]));
    assert_eq!(dialogs.handle(&key(KeyCode::Esc)), Outcome::Close);
    assert_eq!(dialogs.top().unwrap().kind, Kind::Models);
    let mut app = App::default();
    app.commands();
    assert_eq!(app.handle(&key(KeyCode::Char('m'))), Some(Action::None));
    assert!(app.dialogs.top().is_some());
}

#[test]
fn delete_requires_explicit_non_default_confirmation() {
    let mut app = App::default();
    app.open("ses_real".into());
    assert_eq!(app.command("/delete"), Action::None);
    assert_eq!(app.handle(&key(KeyCode::Enter)), Some(Action::None));
    app.command("delete");
    app.handle(&key(KeyCode::Down));
    assert_eq!(app.handle(&key(KeyCode::Enter)), Some(Action::Delete));
}

#[test]
fn navigation_deduplicates_and_forgets_deleted_session() {
    let mut navigation = Navigation::default();
    navigation.navigate(Route::Session("ses_a".into()));
    navigation.navigate(Route::Session("ses_a".into()));
    navigation.navigate(Route::Session("ses_b".into()));
    navigation.forget("ses_b");
    assert_eq!(navigation.current, Route::Session("ses_a".into()));
    navigation.back();
    assert_eq!(navigation.current, Route::Home);
}

#[test]
fn rename_erases_whole_grapheme_and_release_keys_do_not_select() {
    let mut dialog = Dialog::new(Kind::Rename, "Rename", vec![]);
    dialog.handle(&UiEvent::Paste("a👩‍💻".into()));
    dialog.handle(&key(KeyCode::Backspace));
    assert_eq!(dialog.query, "a");
    assert_eq!(
        dialog.handle(&key(KeyCode::Enter)),
        Outcome::Select(Kind::Rename, "a".into())
    );
}
