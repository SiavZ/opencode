//! Purpose: contract tests for safe window URL state and window options.
//! Scope: tests the exact behavior ported from window-state.ts and windows.ts data models.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/windows/state.rs
//! External:
//! `-- serde_json::to_string
//! Used by:
//! `-- none, entrypoint
//! Invariants: safe functions never panic, return constant on error or destroyed state.
//! Contract tests: this file

use opencode_desktop_host::windows::state::{
    DESTROYED_WINDOW_URL, PersistedWindowState, TitleBarOverlay, TitleBarStyle, WebContentsState,
    WindowOptions, WindowState, safe_web_contents_url, safe_window_url,
};

struct MockWebContents {
    destroyed: bool,
    url: String,
}

impl WebContentsState for MockWebContents {
    fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    fn get_url(&self) -> String {
        self.url.clone()
    }
}

struct MockWindow {
    destroyed: bool,
    web_contents: MockWebContents,
}

impl WindowState for MockWindow {
    fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    fn web_contents(&self) -> &dyn WebContentsState {
        &self.web_contents
    }
}

#[test]
fn safe_web_contents_url_returns_destroyed_when_destroyed() {
    let wc = MockWebContents {
        destroyed: true,
        url: "https://example.com".to_string(),
    };
    assert_eq!(safe_web_contents_url(&wc), DESTROYED_WINDOW_URL);
}

#[test]
fn safe_web_contents_url_returns_url_when_not_destroyed() {
    let wc = MockWebContents {
        destroyed: false,
        url: "https://example.com".to_string(),
    };
    assert_eq!(safe_web_contents_url(&wc), "https://example.com");
}

#[test]
fn safe_window_url_returns_destroyed_when_window_destroyed() {
    let win = MockWindow {
        destroyed: true,
        web_contents: MockWebContents {
            destroyed: false,
            url: "https://example.com".to_string(),
        },
    };
    assert_eq!(safe_window_url(&win), DESTROYED_WINDOW_URL);
}

#[test]
fn safe_window_url_returns_destroyed_when_web_contents_destroyed() {
    let win = MockWindow {
        destroyed: false,
        web_contents: MockWebContents {
            destroyed: true,
            url: "https://example.com".to_string(),
        },
    };
    assert_eq!(safe_window_url(&win), DESTROYED_WINDOW_URL);
}

#[test]
fn safe_window_url_returns_url_when_neither_destroyed() {
    let win = MockWindow {
        destroyed: false,
        web_contents: MockWebContents {
            destroyed: false,
            url: "https://example.com".to_string(),
        },
    };
    assert_eq!(safe_window_url(&win), "https://example.com");
}

#[test]
fn window_options_serializes_correctly() {
    let opts = WindowOptions {
        id: "win-1".to_string(),
        width: 1280,
        height: 800,
        x: Some(100),
        y: Some(100),
        min_width: 800,
        min_height: 600,
        background_color: Some("#ffffff".to_string()),
        frame: false,
        title_bar_style: Some(TitleBarStyle::Hidden),
        traffic_light_position: Some((14, 14)),
        title_bar_overlay: Some(TitleBarOverlay {
            color: "#00000000".to_string(),
            symbol_color: "white".to_string(),
            height: 40,
        }),
    };
    let json = serde_json::to_string(&opts).unwrap();
    assert!(json.contains("\"id\":\"win-1\""));
    assert!(json.contains("\"Hidden\""));
}

#[test]
fn persisted_window_state_defaults() {
    let state = PersistedWindowState::default();
    assert_eq!(state.x, None);
    assert_eq!(state.width, 0);
    assert!(!state.is_maximized);
}
