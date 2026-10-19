//! Purpose: verify public prompt editing and display offsets against baseline edge cases.
//! Scope: public model tests, no terminal or user data.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/prompt/mod.rs
//! External:
//! `-- crossterm::event
//! Used by:
//! `-- none, entrypoint
//! Invariants: test the shipped editor, never a duplicate implementation.
//! Contract tests: this file
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use opencode_ui_terminal::{
    input::UiEvent,
    prompt::{Editor, EditorAction, Movement, display_slice, mention_trigger_index, offset_width},
};
fn key(code: KeyCode, modifiers: KeyModifiers) -> UiEvent {
    UiEvent::from(Event::Key(KeyEvent::new(code, modifiers)))
}

#[test]
fn deletes_extended_clusters_without_splitting_utf8() {
    for cluster in ["👨‍👩‍👧‍👦", "e\u{301}", "🇮🇷", "👍🏽", "क्‍ष", "中"]
    {
        let mut editor = Editor::new();
        editor.insert(&format!("a{cluster}b"));
        editor.move_cursor(Movement::Left, false);
        editor.delete(true);
        assert_eq!(editor.text(), "ab", "{cluster}");
        editor.undo();
        assert_eq!(editor.text(), format!("a{cluster}b"));
        editor.move_cursor(Movement::Left, false);
        editor.delete(false);
        assert_eq!(editor.text(), "ab");
    }
}
#[test]
fn selection_replaces_whole_clusters_and_undo_redo_restore() {
    let mut editor = Editor::new();
    editor.insert("a👨‍👩‍👧‍👦中b");
    editor.move_cursor(Movement::Left, false);
    editor.move_cursor(Movement::Left, true);
    editor.move_cursor(Movement::Left, true);
    assert_eq!(&editor.text()[editor.selection().unwrap()], "👨‍👩‍👧‍👦中");
    editor.insert("é");
    assert_eq!(editor.text(), "aéb");
    editor.undo();
    assert_eq!(editor.text(), "a👨‍👩‍👧‍👦中b");
    editor.redo();
    assert_eq!(editor.text(), "aéb");
    editor.select_all();
    editor.delete(false);
    assert_eq!(editor.text(), "");
}
#[test]
fn paste_normalizes_crlf_and_cr_and_never_submits() {
    let mut editor = Editor::new();
    assert_eq!(
        editor.handle(&UiEvent::Paste("one\r\n中文\rthree\n".into())),
        EditorAction::Changed
    );
    assert_eq!(editor.text(), "one\n中文\nthree\n");
    editor.undo();
    assert!(editor.text().is_empty());
    editor.redo();
    let expected = editor.text().to_owned();
    assert_eq!(
        editor.handle(&key(KeyCode::Enter, KeyModifiers::NONE)),
        EditorAction::Submit(expected)
    );
    assert_eq!(editor.text(), "");
    assert_eq!(
        editor.handle(&key(KeyCode::Enter, KeyModifiers::NONE)),
        EditorAction::None
    );
}
#[test]
fn shifted_enter_and_release_keys_do_not_accidentally_submit() {
    let mut editor = Editor::new();
    editor.insert("hello");
    editor.handle(&key(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(editor.text(), "hello\n");
    let mut release = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    release.kind = crossterm::event::KeyEventKind::Release;
    assert_eq!(
        editor.handle(&UiEvent::from(Event::Key(release))),
        EditorAction::None
    );
    assert_eq!(editor.text(), "hello\n");
}
#[test]
fn preferred_display_column_survives_short_lines_and_wide_clusters() {
    let mut editor = Editor::new();
    editor.set_text("abcd\nx\n中文ab");
    editor.move_cursor(Movement::Start, false);
    editor.move_cursor(Movement::Right, false);
    editor.move_cursor(Movement::Right, false);
    editor.move_cursor(Movement::Right, false);
    editor.move_cursor(Movement::Down, false);
    assert_eq!(&editor.text()[..editor.cursor()], "abcd\nx");
    editor.move_cursor(Movement::Down, false);
    assert_eq!(&editor.text()[editor.cursor()..], "文ab");
    editor.move_cursor(Movement::Up, false);
    editor.move_cursor(Movement::Up, false);
    assert_eq!(editor.cursor(), 3);
}
#[test]
fn layout_wraps_wide_clusters_and_tracks_cell_cursor() {
    let mut editor = Editor::new();
    editor.set_text("a中👨‍👩‍👧‍👦e\u{301}");
    let layout = editor.layout(4);
    assert_eq!(
        layout
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["a中", "👨‍👩‍👧‍👦e\u{301}"]
    );
    assert_eq!(layout.cursor, (3, 1));
    editor.move_cursor(Movement::Left, true);
    let layout = editor.layout(4);
    assert!(
        layout.lines[1]
            .spans
            .last()
            .unwrap()
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED)
    );
}
#[test]
fn baseline_display_offset_mentions_include_newline_positions() {
    for (value, expected) in [
        ("@", Some(0)),
        ("中文 @", Some(5)),
        ("こんにちは @", Some(11)),
        ("한국어 @", Some(7)),
        ("👨‍👩‍👧‍👦 @src", Some(3)),
        ("中文 @one\n@two", Some(10)),
        ("hello@", None),
        ("中文@", None),
        ("中文 @src file", None),
    ] {
        assert_eq!(
            mention_trigger_index(value, offset_width(value)),
            expected,
            "{value}"
        );
    }
    assert_eq!(display_slice("中文 @src", 5, 9), "@src");
    assert_eq!(display_slice("中文 @one\n@two", 11, 14), "two");
    assert_eq!(display_slice("👨‍👩‍👧‍👦x", 1, 2), "👨‍👩‍👧‍👦");
}
#[test]
fn history_dedupes_consecutive_entries_and_protects_edited_drafts() {
    let mut editor = Editor::new();
    editor.remember("one".into());
    editor.remember("one".into());
    editor.remember("two".into());
    assert!(editor.history_move(true));
    assert_eq!(editor.text(), "two");
    assert_eq!(editor.cursor(), 0);
    assert!(editor.history_move(true));
    assert_eq!(editor.text(), "one");
    assert!(editor.history_move(false));
    assert_eq!(editor.text(), "two");
    editor.insert("draft");
    assert!(!editor.history_move(true));
    assert_eq!(editor.text(), "twodraft");
    editor.set_text("");
    for index in 0..55 {
        editor.remember(index.to_string());
    }
    for _ in 0..60 {
        editor.history_move(true);
    }
    assert_eq!(editor.text(), "5");
}
#[test]
fn inserting_combining_text_reestablishes_grapheme_boundaries() {
    let mut editor = Editor::new();
    editor.insert("a\u{301}");
    editor.move_cursor(Movement::Start, false);
    editor.insert("e");
    editor.delete(false);
    assert_eq!(editor.text(), "e");
    editor.insert("\u{301}");
    editor.delete(true);
    assert_eq!(editor.text(), "");
}
