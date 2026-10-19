//! Purpose: verify crossterm event to UiEvent conversion covers all variants and properties.
//! Scope: unit tests for input mapping.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/input.rs
//! External:
//! |-- crossterm::event::Event
//! |-- crossterm::event::KeyCode
//! |-- crossterm::event::KeyEvent
//! |-- crossterm::event::KeyEventKind
//! |-- crossterm::event::KeyEventState
//! |-- crossterm::event::KeyModifiers
//! |-- crossterm::event::MediaKeyCode
//! |-- crossterm::event::ModifierKeyCode
//! |-- crossterm::event::MouseButton
//! |-- crossterm::event::MouseEvent
//! `-- crossterm::event::MouseEventKind
//! Used by:
//! `-- none, entrypoint
//! Invariants: all crossterm event variants map to a distinct UiEvent variant without data loss.
//! Contract tests: ui-terminal/tests/input.rs

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use opencode_ui_terminal::input::{
    UiEvent, UiKeyCode, UiKeyEventKind, UiMediaKeyCode, UiModifierKeyCode, UiMouseButton,
    UiMouseEventKind,
};

#[test]
fn test_key_event_conversion() {
    let key_event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
    let ui_event: UiEvent = Event::Key(key_event).into();

    assert!(matches!(ui_event, UiEvent::Key(_)));
    if let UiEvent::Key(ui_key) = ui_event {
        assert_eq!(ui_key.code, UiKeyCode::Char('a'));
        assert!(ui_key.modifiers.control);
        assert!(!ui_key.modifiers.shift);
        assert_eq!(ui_key.kind, UiKeyEventKind::Press);
    }
}

#[test]
fn test_function_key_conversion() {
    let key_event = KeyEvent::new(KeyCode::F(12), KeyModifiers::SHIFT);
    let ui_event: UiEvent = Event::Key(key_event).into();

    if let UiEvent::Key(ui_key) = ui_event {
        assert_eq!(ui_key.code, UiKeyCode::F(12));
        assert!(ui_key.modifiers.shift);
    }
}

#[test]
fn test_media_key_conversion() {
    let key_event = KeyEvent::new(
        KeyCode::Media(MediaKeyCode::PlayPause),
        KeyModifiers::empty(),
    );
    let ui_event: UiEvent = Event::Key(key_event).into();

    if let UiEvent::Key(ui_key) = ui_event {
        assert_eq!(ui_key.code, UiKeyCode::Media(UiMediaKeyCode::PlayPause));
    }
}

#[test]
fn test_modifier_key_conversion() {
    let key_event = KeyEvent::new(
        KeyCode::Modifier(ModifierKeyCode::LeftControl),
        KeyModifiers::CONTROL,
    );
    let ui_event: UiEvent = Event::Key(key_event).into();

    if let UiEvent::Key(ui_key) = ui_event {
        assert_eq!(
            ui_key.code,
            UiKeyCode::Modifier(UiModifierKeyCode::LeftControl)
        );
    }
}

#[test]
fn test_key_event_kind_and_state() {
    let mut state = KeyEventState::empty();
    state.insert(KeyEventState::CAPS_LOCK);
    state.insert(KeyEventState::KEYPAD);

    let key_event = KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Repeat,
        state,
    };
    let ui_event: UiEvent = Event::Key(key_event).into();

    if let UiEvent::Key(ui_key) = ui_event {
        assert_eq!(ui_key.kind, UiKeyEventKind::Repeat);
        assert!(ui_key.state.caps_lock);
        assert!(ui_key.state.keypad);
        assert!(!ui_key.state.num_lock);
    }
}

#[test]
fn test_mouse_event_conversion() {
    let mouse_event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 10,
        row: 20,
        modifiers: KeyModifiers::ALT,
    };
    let ui_event: UiEvent = Event::Mouse(mouse_event).into();

    assert!(matches!(ui_event, UiEvent::Mouse(_)));
    if let UiEvent::Mouse(ui_mouse) = ui_event {
        assert_eq!(ui_mouse.kind, UiMouseEventKind::Down(UiMouseButton::Right));
        assert_eq!(ui_mouse.column, 10);
        assert_eq!(ui_mouse.row, 20);
        assert!(ui_mouse.modifiers.alt);
    }
}

#[test]
fn test_mouse_scroll_conversion() {
    let mouse_event = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::empty(),
    };
    let ui_event: UiEvent = Event::Mouse(mouse_event).into();

    if let UiEvent::Mouse(ui_mouse) = ui_event {
        assert_eq!(ui_mouse.kind, UiMouseEventKind::ScrollUp);
    }
}

#[test]
fn test_paste_event_conversion() {
    let text = "pasted text\nwith newlines".to_string();
    let ui_event: UiEvent = Event::Paste(text.clone()).into();

    assert_eq!(ui_event, UiEvent::Paste(text));
}

#[test]
fn test_resize_event_conversion() {
    let ui_event: UiEvent = Event::Resize(80, 24).into();

    assert_eq!(ui_event, UiEvent::Resize(80, 24));
}

#[test]
fn test_focus_events_conversion() {
    let gained: UiEvent = Event::FocusGained.into();
    let lost: UiEvent = Event::FocusLost.into();

    assert_eq!(gained, UiEvent::FocusGained);
    assert_eq!(lost, UiEvent::FocusLost);
}

#[test]
fn test_all_key_codes_map() {
    let codes = [
        KeyCode::Backspace,
        KeyCode::Enter,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::F(1),
        KeyCode::Char('x'),
        KeyCode::Null,
        KeyCode::Esc,
        KeyCode::CapsLock,
        KeyCode::ScrollLock,
        KeyCode::NumLock,
        KeyCode::PrintScreen,
        KeyCode::Pause,
        KeyCode::Menu,
        KeyCode::KeypadBegin,
    ];

    for code in codes {
        let key_event = KeyEvent::new(code, KeyModifiers::empty());
        let ui_event: UiEvent = Event::Key(key_event).into();
        assert!(matches!(ui_event, UiEvent::Key(_)));
    }
}

#[test]
fn test_all_media_codes_map() {
    let media_codes = [
        MediaKeyCode::Play,
        MediaKeyCode::Pause,
        MediaKeyCode::PlayPause,
        MediaKeyCode::Reverse,
        MediaKeyCode::Stop,
        MediaKeyCode::FastForward,
        MediaKeyCode::Rewind,
        MediaKeyCode::TrackNext,
        MediaKeyCode::TrackPrevious,
        MediaKeyCode::Record,
        MediaKeyCode::LowerVolume,
        MediaKeyCode::RaiseVolume,
        MediaKeyCode::MuteVolume,
    ];

    for code in media_codes {
        let key_event = KeyEvent::new(KeyCode::Media(code), KeyModifiers::empty());
        let ui_event: UiEvent = Event::Key(key_event).into();
        assert!(matches!(ui_event, UiEvent::Key(_)));
    }
}

#[test]
fn test_all_modifier_codes_map() {
    let modifier_codes = [
        ModifierKeyCode::LeftShift,
        ModifierKeyCode::LeftControl,
        ModifierKeyCode::LeftAlt,
        ModifierKeyCode::LeftSuper,
        ModifierKeyCode::LeftHyper,
        ModifierKeyCode::LeftMeta,
        ModifierKeyCode::RightShift,
        ModifierKeyCode::RightControl,
        ModifierKeyCode::RightAlt,
        ModifierKeyCode::RightSuper,
        ModifierKeyCode::RightHyper,
        ModifierKeyCode::RightMeta,
        ModifierKeyCode::IsoLevel3Shift,
        ModifierKeyCode::IsoLevel5Shift,
    ];

    for code in modifier_codes {
        let key_event = KeyEvent::new(KeyCode::Modifier(code), KeyModifiers::empty());
        let ui_event: UiEvent = Event::Key(key_event).into();
        assert!(matches!(ui_event, UiEvent::Key(_)));
    }
}

#[test]
fn test_all_mouse_event_kinds_map() {
    let mouse_kinds = [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Middle),
        MouseEventKind::Drag(MouseButton::Right),
        MouseEventKind::Moved,
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollUp,
        MouseEventKind::ScrollLeft,
        MouseEventKind::ScrollRight,
    ];

    for kind in mouse_kinds {
        let mouse_event = MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::empty(),
        };
        let ui_event: UiEvent = Event::Mouse(mouse_event).into();
        assert!(matches!(ui_event, UiEvent::Mouse(_)));
    }
}

#[test]
fn test_key_modifiers_combinations() {
    let modifiers = KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT;
    let key_event = KeyEvent::new(KeyCode::Char('c'), modifiers);
    let ui_event: UiEvent = Event::Key(key_event).into();

    if let UiEvent::Key(ui_key) = ui_event {
        assert!(ui_key.modifiers.shift);
        assert!(ui_key.modifiers.control);
        assert!(ui_key.modifiers.alt);
        assert!(!ui_key.modifiers.super_key);
        assert!(!ui_key.modifiers.hyper);
        assert!(!ui_key.modifiers.meta);
    }
}
