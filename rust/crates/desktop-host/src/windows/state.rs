//! Purpose: safe window URL state retrieval and window creation options.
//! Scope: provides fallback for destroyed windows and platform-independent window options.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/windows.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/windows.rs
//! Invariants: safe functions never panic, return constant on error or destroyed state.
//! Contract tests: desktop-host/tests/windows_state.rs

pub const DESTROYED_WINDOW_URL: &str = "<destroyed>";

pub trait WebContentsState {
    fn is_destroyed(&self) -> bool;
    fn get_url(&self) -> String;
}

pub trait WindowState {
    fn is_destroyed(&self) -> bool;
    fn web_contents(&self) -> &dyn WebContentsState;
}

pub fn safe_web_contents_url(web_contents: &dyn WebContentsState) -> String {
    if web_contents.is_destroyed() {
        return DESTROYED_WINDOW_URL.to_string();
    }
    web_contents.get_url()
}

pub fn safe_window_url(win: &dyn WindowState) -> String {
    if win.is_destroyed() {
        return DESTROYED_WINDOW_URL.to_string();
    }
    safe_web_contents_url(win.web_contents())
}

/// Persisted window state (bounds, maximized, fullscreen).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct PersistedWindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: i32,
    pub height: i32,
    pub is_maximized: bool,
    pub is_fullscreen: bool,
}

/// Platform-independent window creation options.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowOptions {
    pub id: String,
    pub width: i32,
    pub height: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub min_width: i32,
    pub min_height: i32,
    pub background_color: Option<String>,
    pub frame: bool,
    pub title_bar_style: Option<TitleBarStyle>,
    pub traffic_light_position: Option<(i32, i32)>,
    pub title_bar_overlay: Option<TitleBarOverlay>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TitleBarStyle {
    Hidden,
    HiddenInset,
    CustomButtonsOnHover,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TitleBarOverlay {
    pub color: String,
    pub symbol_color: String,
    pub height: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZoomState {
    pub factor: f64,
    pub pinch_zoom_enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FullscreenState {
    pub is_fullscreen: bool,
}
