//! Purpose: normalize crossterm events into typed UI input (keys, paste, mouse, focus, resize).
//! Scope: per event conversion.
//! Uses:
//! `-- none
//! External:
//! `-- crossterm::event::{Event, KeyEvent, MouseEvent, KeyCode, KeyModifiers, KeyEventKind, KeyEventState, MouseEventKind, MouseButton, MediaKeyCode, ModifierKeyCode}
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/terminal.rs
//! Invariants: no input is dropped silently, all crossterm events map to a UiEvent variant.
//! Contract tests: ui-terminal/tests/input.rs

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

/// Typed UI event normalized from crossterm events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Key(UiKeyEvent),
    Mouse(UiMouseEvent),
    Paste(String),
    Resize(u16, u16),
    FocusGained,
    FocusLost,
}

/// Typed key event with disambiguated properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiKeyEvent {
    pub code: UiKeyCode,
    pub modifiers: UiKeyModifiers,
    pub kind: UiKeyEventKind,
    pub state: UiKeyEventState,
}

/// Normalized key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Null,
    Esc,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    KeypadBegin,
    Media(UiMediaKeyCode),
    Modifier(UiModifierKeyCode),
}

/// Normalized media key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMediaKeyCode {
    Play,
    Pause,
    PlayPause,
    Reverse,
    Stop,
    FastForward,
    Rewind,
    TrackNext,
    TrackPrevious,
    Record,
    LowerVolume,
    RaiseVolume,
    MuteVolume,
}

/// Normalized modifier key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiModifierKeyCode {
    LeftShift,
    LeftControl,
    LeftAlt,
    LeftSuper,
    LeftHyper,
    LeftMeta,
    RightShift,
    RightControl,
    RightAlt,
    RightSuper,
    RightHyper,
    RightMeta,
    IsoLevel3Shift,
    IsoLevel5Shift,
}

/// Key modifiers bitmask representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiKeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
    pub hyper: bool,
    pub meta: bool,
}

/// Key event kind (press, repeat, release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKeyEventKind {
    Press,
    Repeat,
    Release,
}

/// Key event state flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiKeyEventState {
    pub keypad: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
}

/// Typed mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiMouseEvent {
    pub kind: UiMouseEventKind,
    pub column: u16,
    pub row: u16,
    pub modifiers: UiKeyModifiers,
}

/// Mouse event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMouseEventKind {
    Down(UiMouseButton),
    Up(UiMouseButton),
    Drag(UiMouseButton),
    Moved,
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

/// Mouse button variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMouseButton {
    Left,
    Right,
    Middle,
}

impl From<Event> for UiEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::Key(key) => UiEvent::Key(UiKeyEvent::from(key)),
            Event::Mouse(mouse) => UiEvent::Mouse(UiMouseEvent::from(mouse)),
            Event::Paste(text) => UiEvent::Paste(text),
            Event::Resize(cols, rows) => UiEvent::Resize(cols, rows),
            Event::FocusGained => UiEvent::FocusGained,
            Event::FocusLost => UiEvent::FocusLost,
        }
    }
}

impl From<KeyEvent> for UiKeyEvent {
    fn from(key: KeyEvent) -> Self {
        Self {
            code: UiKeyCode::from(key.code),
            modifiers: UiKeyModifiers::from(key.modifiers),
            kind: UiKeyEventKind::from(key.kind),
            state: UiKeyEventState::from(key.state),
        }
    }
}

impl From<KeyCode> for UiKeyCode {
    fn from(code: KeyCode) -> Self {
        match code {
            KeyCode::Backspace => UiKeyCode::Backspace,
            KeyCode::Enter => UiKeyCode::Enter,
            KeyCode::Left => UiKeyCode::Left,
            KeyCode::Right => UiKeyCode::Right,
            KeyCode::Up => UiKeyCode::Up,
            KeyCode::Down => UiKeyCode::Down,
            KeyCode::Home => UiKeyCode::Home,
            KeyCode::End => UiKeyCode::End,
            KeyCode::PageUp => UiKeyCode::PageUp,
            KeyCode::PageDown => UiKeyCode::PageDown,
            KeyCode::Tab => UiKeyCode::Tab,
            KeyCode::BackTab => UiKeyCode::BackTab,
            KeyCode::Delete => UiKeyCode::Delete,
            KeyCode::Insert => UiKeyCode::Insert,
            KeyCode::F(n) => UiKeyCode::F(n),
            KeyCode::Char(c) => UiKeyCode::Char(c),
            KeyCode::Null => UiKeyCode::Null,
            KeyCode::Esc => UiKeyCode::Esc,
            KeyCode::CapsLock => UiKeyCode::CapsLock,
            KeyCode::ScrollLock => UiKeyCode::ScrollLock,
            KeyCode::NumLock => UiKeyCode::NumLock,
            KeyCode::PrintScreen => UiKeyCode::PrintScreen,
            KeyCode::Pause => UiKeyCode::Pause,
            KeyCode::Menu => UiKeyCode::Menu,
            KeyCode::KeypadBegin => UiKeyCode::KeypadBegin,
            KeyCode::Media(m) => UiKeyCode::Media(UiMediaKeyCode::from(m)),
            KeyCode::Modifier(m) => UiKeyCode::Modifier(UiModifierKeyCode::from(m)),
        }
    }
}

impl From<MediaKeyCode> for UiMediaKeyCode {
    fn from(m: MediaKeyCode) -> Self {
        match m {
            MediaKeyCode::Play => UiMediaKeyCode::Play,
            MediaKeyCode::Pause => UiMediaKeyCode::Pause,
            MediaKeyCode::PlayPause => UiMediaKeyCode::PlayPause,
            MediaKeyCode::Reverse => UiMediaKeyCode::Reverse,
            MediaKeyCode::Stop => UiMediaKeyCode::Stop,
            MediaKeyCode::FastForward => UiMediaKeyCode::FastForward,
            MediaKeyCode::Rewind => UiMediaKeyCode::Rewind,
            MediaKeyCode::TrackNext => UiMediaKeyCode::TrackNext,
            MediaKeyCode::TrackPrevious => UiMediaKeyCode::TrackPrevious,
            MediaKeyCode::Record => UiMediaKeyCode::Record,
            MediaKeyCode::LowerVolume => UiMediaKeyCode::LowerVolume,
            MediaKeyCode::RaiseVolume => UiMediaKeyCode::RaiseVolume,
            MediaKeyCode::MuteVolume => UiMediaKeyCode::MuteVolume,
        }
    }
}

impl From<ModifierKeyCode> for UiModifierKeyCode {
    fn from(m: ModifierKeyCode) -> Self {
        match m {
            ModifierKeyCode::LeftShift => UiModifierKeyCode::LeftShift,
            ModifierKeyCode::LeftControl => UiModifierKeyCode::LeftControl,
            ModifierKeyCode::LeftAlt => UiModifierKeyCode::LeftAlt,
            ModifierKeyCode::LeftSuper => UiModifierKeyCode::LeftSuper,
            ModifierKeyCode::LeftHyper => UiModifierKeyCode::LeftHyper,
            ModifierKeyCode::LeftMeta => UiModifierKeyCode::LeftMeta,
            ModifierKeyCode::RightShift => UiModifierKeyCode::RightShift,
            ModifierKeyCode::RightControl => UiModifierKeyCode::RightControl,
            ModifierKeyCode::RightAlt => UiModifierKeyCode::RightAlt,
            ModifierKeyCode::RightSuper => UiModifierKeyCode::RightSuper,
            ModifierKeyCode::RightHyper => UiModifierKeyCode::RightHyper,
            ModifierKeyCode::RightMeta => UiModifierKeyCode::RightMeta,
            ModifierKeyCode::IsoLevel3Shift => UiModifierKeyCode::IsoLevel3Shift,
            ModifierKeyCode::IsoLevel5Shift => UiModifierKeyCode::IsoLevel5Shift,
        }
    }
}

impl From<KeyModifiers> for UiKeyModifiers {
    fn from(mods: KeyModifiers) -> Self {
        Self {
            shift: mods.contains(KeyModifiers::SHIFT),
            control: mods.contains(KeyModifiers::CONTROL),
            alt: mods.contains(KeyModifiers::ALT),
            super_key: mods.contains(KeyModifiers::SUPER),
            hyper: mods.contains(KeyModifiers::HYPER),
            meta: mods.contains(KeyModifiers::META),
        }
    }
}

impl From<KeyEventKind> for UiKeyEventKind {
    fn from(kind: KeyEventKind) -> Self {
        match kind {
            KeyEventKind::Press => UiKeyEventKind::Press,
            KeyEventKind::Repeat => UiKeyEventKind::Repeat,
            KeyEventKind::Release => UiKeyEventKind::Release,
        }
    }
}

impl From<KeyEventState> for UiKeyEventState {
    fn from(state: KeyEventState) -> Self {
        Self {
            keypad: state.contains(KeyEventState::KEYPAD),
            caps_lock: state.contains(KeyEventState::CAPS_LOCK),
            num_lock: state.contains(KeyEventState::NUM_LOCK),
            scroll_lock: false, // crossterm 0.29.0 does not expose scroll lock state
        }
    }
}

impl From<MouseEvent> for UiMouseEvent {
    fn from(mouse: MouseEvent) -> Self {
        Self {
            kind: UiMouseEventKind::from(mouse.kind),
            column: mouse.column,
            row: mouse.row,
            modifiers: UiKeyModifiers::from(mouse.modifiers),
        }
    }
}

impl From<MouseEventKind> for UiMouseEventKind {
    fn from(kind: MouseEventKind) -> Self {
        match kind {
            MouseEventKind::Down(button) => UiMouseEventKind::Down(UiMouseButton::from(button)),
            MouseEventKind::Up(button) => UiMouseEventKind::Up(UiMouseButton::from(button)),
            MouseEventKind::Drag(button) => UiMouseEventKind::Drag(UiMouseButton::from(button)),
            MouseEventKind::Moved => UiMouseEventKind::Moved,
            MouseEventKind::ScrollDown => UiMouseEventKind::ScrollDown,
            MouseEventKind::ScrollUp => UiMouseEventKind::ScrollUp,
            MouseEventKind::ScrollLeft => UiMouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight => UiMouseEventKind::ScrollRight,
        }
    }
}

impl From<MouseButton> for UiMouseButton {
    fn from(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => UiMouseButton::Left,
            MouseButton::Right => UiMouseButton::Right,
            MouseButton::Middle => UiMouseButton::Middle,
        }
    }
}
