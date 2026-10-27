//! Purpose: exercise target-owned browser draft admission and rollback.
//! Scope: public portable interaction model.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: navigation and asynchronous failures never overwrite another target or newer input.
//! Contract tests: this file
use opencode_view_model::browser::{Draft, DraftBook, EnterAction, enter_action};

#[test]
fn captured_failure_restores_original_target_not_visible_target() {
    let mut book = DraftBook::default();
    book.edit(
        "server:a/session:1",
        Draft {
            text: "first".into(),
            ..Draft::default()
        },
    );
    let submission = book.begin("server:a/session:1").unwrap();
    book.edit(
        "server:b/session:2",
        Draft {
            text: "other".into(),
            ..Draft::default()
        },
    );
    assert!(book.finish(&submission, false));
    assert_eq!(book.get("server:a/session:1").text, "first");
    assert_eq!(book.get("server:b/session:2").text, "other");
}

#[test]
fn failure_does_not_clobber_new_typing_and_duplicate_submission_is_blocked() {
    let mut book = DraftBook::default();
    book.edit(
        "a",
        Draft {
            text: "old".into(),
            ..Draft::default()
        },
    );
    let submission = book.begin("a").unwrap();
    assert!(book.begin("a").is_none());
    book.edit(
        "a",
        Draft {
            text: "new".into(),
            ..Draft::default()
        },
    );
    assert!(!book.finish(&submission, false));
    assert_eq!(book.get("a").text, "new");
    assert!(!book.busy("a"));
    assert!(!book.finish(&submission, false));
}

#[test]
fn whitespace_is_not_sent_and_success_preserves_new_draft() {
    let mut book = DraftBook::default();
    book.edit(
        "a",
        Draft {
            text: " \n".into(),
            ..Draft::default()
        },
    );
    assert!(book.begin("a").is_none());
    book.edit(
        "a",
        Draft {
            text: "send".into(),
            ..Draft::default()
        },
    );
    let submission = book.begin("a").unwrap();
    book.edit(
        "a",
        Draft {
            text: "next".into(),
            ..Draft::default()
        },
    );
    assert!(!book.finish(&submission, true));
    assert_eq!(book.get("a").text, "next");
}

#[test]
fn shift_enter_precedes_ime_and_legacy_229_never_submits() {
    assert_eq!(enter_action(true, true, 229, false), EnterAction::Newline);
    assert_eq!(enter_action(false, true, 13, false), EnterAction::Compose);
    assert_eq!(enter_action(false, false, 229, false), EnterAction::Compose);
    assert_eq!(
        enter_action(false, false, 13, true),
        EnterAction::IgnoreRepeat
    );
    assert_eq!(enter_action(false, false, 13, false), EnterAction::Submit);
}
