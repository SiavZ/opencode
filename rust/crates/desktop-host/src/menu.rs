//! Purpose: native menu model, action routing, and compatibility exports.
//! Scope: toolkit-independent menu types and dispatch; construction and translations live in private modules.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/menu_catalog.rs
//! |-- desktop-host/src/menu_english.rs
//! `-- desktop-host/src/menu_translation.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lib.rs
//! `-- desktop-host/src/menu_catalog.rs
//! Invariants: public menu paths stay stable; dispatch sends actions before explicit commands.
//! Contract tests: desktop-host/tests/menu_catalog.rs, menu_translations.rs, menu_actions.rs

#[path = "menu_catalog.rs"]
mod menu_catalog;
#[path = "menu_english.rs"]
mod menu_english;
#[path = "menu_translation.rs"]
mod menu_translation;

pub use menu_catalog::{MenuItemBuilder, build_menu_catalog};
pub use menu_english::english_messages;
pub use menu_translation::{TranslationBundle, TranslationManager, format_message};

use serde::{Deserialize, Serialize};

/// Platform identifier for menu layout differences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
}

/// Desktop menu action matching TypeScript DesktopMenuAction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuAction {
    AppCheckForUpdates,
    AppRelaunch,
    EditUndo,
    EditRedo,
    EditCut,
    EditCopy,
    EditPaste,
    EditDelete,
    EditSelectAll,
    ViewReload,
    ViewToggleDevTools,
    ViewResetZoom,
    ViewZoomIn,
    ViewZoomOut,
    ViewToggleFullscreen,
    WindowNew,
    WindowClose,
    WindowMinimize,
    WindowToggleMaximize,
}

impl MenuAction {
    /// Convert action to its command string for IPC routing.
    pub fn as_command_str(&self) -> &'static str {
        match self {
            MenuAction::AppCheckForUpdates => "app.checkForUpdates",
            MenuAction::AppRelaunch => "app.relaunch",
            MenuAction::EditUndo => "edit.undo",
            MenuAction::EditRedo => "edit.redo",
            MenuAction::EditCut => "edit.cut",
            MenuAction::EditCopy => "edit.copy",
            MenuAction::EditPaste => "edit.paste",
            MenuAction::EditDelete => "edit.delete",
            MenuAction::EditSelectAll => "edit.selectAll",
            MenuAction::ViewReload => "view.reload",
            MenuAction::ViewToggleDevTools => "view.toggleDevTools",
            MenuAction::ViewResetZoom => "view.resetZoom",
            MenuAction::ViewZoomIn => "view.zoomIn",
            MenuAction::ViewZoomOut => "view.zoomOut",
            MenuAction::ViewToggleFullscreen => "view.toggleFullscreen",
            MenuAction::WindowNew => "window.new",
            MenuAction::WindowClose => "window.close",
            MenuAction::WindowMinimize => "window.minimize",
            MenuAction::WindowToggleMaximize => "window.toggleMaximize",
        }
    }

    /// Parse action from command string.
    pub fn from_command_str(s: &str) -> Option<Self> {
        match s {
            "app.checkForUpdates" => Some(MenuAction::AppCheckForUpdates),
            "app.relaunch" => Some(MenuAction::AppRelaunch),
            "edit.undo" => Some(MenuAction::EditUndo),
            "edit.redo" => Some(MenuAction::EditRedo),
            "edit.cut" => Some(MenuAction::EditCut),
            "edit.copy" => Some(MenuAction::EditCopy),
            "edit.paste" => Some(MenuAction::EditPaste),
            "edit.delete" => Some(MenuAction::EditDelete),
            "edit.selectAll" => Some(MenuAction::EditSelectAll),
            "view.reload" => Some(MenuAction::ViewReload),
            "view.toggleDevTools" => Some(MenuAction::ViewToggleDevTools),
            "view.resetZoom" => Some(MenuAction::ViewResetZoom),
            "view.zoomIn" => Some(MenuAction::ViewZoomIn),
            "view.zoomOut" => Some(MenuAction::ViewZoomOut),
            "view.toggleFullscreen" => Some(MenuAction::ViewToggleFullscreen),
            "window.new" => Some(MenuAction::WindowNew),
            "window.close" => Some(MenuAction::WindowClose),
            "window.minimize" => Some(MenuAction::WindowMinimize),
            "window.toggleMaximize" => Some(MenuAction::WindowToggleMaximize),
            _ => None,
        }
    }

    /// All action variants for exhaustive testing.
    pub const ALL: &'static [MenuAction] = &[
        MenuAction::AppCheckForUpdates,
        MenuAction::AppRelaunch,
        MenuAction::EditUndo,
        MenuAction::EditRedo,
        MenuAction::EditCut,
        MenuAction::EditCopy,
        MenuAction::EditPaste,
        MenuAction::EditDelete,
        MenuAction::EditSelectAll,
        MenuAction::ViewReload,
        MenuAction::ViewToggleDevTools,
        MenuAction::ViewResetZoom,
        MenuAction::ViewZoomIn,
        MenuAction::ViewZoomOut,
        MenuAction::ViewToggleFullscreen,
        MenuAction::WindowNew,
        MenuAction::WindowClose,
        MenuAction::WindowMinimize,
        MenuAction::WindowToggleMaximize,
    ];
}

/// Desktop menu role matching TypeScript DesktopMenuRole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MenuRole {
    About,
    Close,
    Copy,
    Cut,
    Hide,
    HideOthers,
    Paste,
    Quit,
    Redo,
    Reload,
    ResetZoom,
    SelectAll,
    ToggleDevTools,
    ToggleFullscreen,
    Undo,
    Unhide,
    WindowMenu,
    ZoomIn,
    ZoomOut,
}

/// Accelerator key bindings per platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accelerators {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
}

impl Accelerators {
    /// Get accelerator for specific platform.
    pub fn for_platform(&self, platform: Platform) -> Option<&str> {
        match platform {
            Platform::MacOS => self.macos.as_deref(),
            Platform::Windows => self.windows.as_deref(),
            Platform::Linux => self.linux.as_deref(),
        }
    }
}

/// Enabled condition for menu items.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnabledCondition {
    /// Always enabled (default).
    #[default]
    Always,
    /// Enabled only when updater is available.
    Updater,
}

/// A single menu entry: either an item or a separator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MenuEntry {
    /// Regular menu item.
    #[serde(rename = "item")]
    Item {
        /// Translation key for label.
        #[serde(skip_serializing_if = "Option::is_none")]
        label_key: Option<String>,
        /// Command string sent via IPC when clicked.
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        /// Desktop menu action dispatched to handlers.
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<MenuAction>,
        /// Native menu role (e.g. about, quit).
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<MenuRole>,
        /// URL to open in default browser.
        #[serde(skip_serializing_if = "Option::is_none")]
        href: Option<String>,
        /// Keyboard accelerators per platform.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accelerator: Option<Accelerators>,
        /// Enabled condition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<EnabledCondition>,
        /// Platform visibility filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platforms: Option<Vec<Platform>>,
    },
    /// Separator line.
    #[serde(rename = "separator")]
    Separator {
        /// Platform visibility filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platforms: Option<Vec<Platform>>,
    },
}

impl MenuEntry {
    /// Check if entry is visible on given platform.
    pub fn is_visible_on(&self, platform: Platform) -> bool {
        let platforms = match self {
            MenuEntry::Item { platforms, .. } => platforms,
            MenuEntry::Separator { platforms } => platforms,
        };
        platforms.as_ref().is_none_or(|p| p.contains(&platform))
    }

    /// Get the command string if this is an item with a command.
    pub fn command(&self) -> Option<&str> {
        match self {
            MenuEntry::Item { command, .. } => command.as_deref(),
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Get the action if this is an item with an action.
    pub fn action(&self) -> Option<MenuAction> {
        match self {
            MenuEntry::Item { action, .. } => *action,
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Get the href if this is an item with a URL.
    pub fn href(&self) -> Option<&str> {
        match self {
            MenuEntry::Item { href, .. } => href.as_deref(),
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Get the role if this is an item with a role.
    pub fn role(&self) -> Option<MenuRole> {
        match self {
            MenuEntry::Item { role, .. } => *role,
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Get the label key if this is an item with a label key.
    pub fn label_key(&self) -> Option<&str> {
        match self {
            MenuEntry::Item { label_key, .. } => label_key.as_deref(),
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Get the accelerator for a specific platform.
    pub fn accelerator_for(&self, platform: Platform) -> Option<&str> {
        match self {
            MenuEntry::Item { accelerator, .. } => {
                accelerator.as_ref().and_then(|a| a.for_platform(platform))
            }
            MenuEntry::Separator { .. } => None,
        }
    }

    /// Check if this is a separator.
    pub fn is_separator(&self) -> bool {
        matches!(self, MenuEntry::Separator { .. })
    }
}

/// Top-level menu (e.g., File, Edit, View).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    /// Menu identifier.
    pub id: String,
    /// Translation key for label.
    pub label_key: String,
    /// Native menu role (for special menus like windowMenu).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<MenuRole>,
    /// Menu items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<MenuEntry>>,
    /// Platform visibility filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<Platform>>,
}

impl Menu {
    /// Check if menu is visible on given platform.
    pub fn is_visible_on(&self, platform: Platform) -> bool {
        self.platforms
            .as_ref()
            .is_none_or(|p| p.contains(&platform))
    }

    /// Get items visible on specific platform.
    pub fn visible_items(&self, platform: Platform) -> Vec<&MenuEntry> {
        self.items
            .as_ref()
            .map(|items| items.iter().filter(|i| i.is_visible_on(platform)).collect())
            .unwrap_or_default()
    }
}

/// Complete menu catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuCatalog {
    /// Top-level menus.
    pub menus: Vec<Menu>,
}

impl MenuCatalog {
    /// Get menus visible on specific platform.
    pub fn visible_menus(&self, platform: Platform) -> Vec<&Menu> {
        self.menus
            .iter()
            .filter(|m| m.is_visible_on(platform))
            .collect()
    }
}

/// Trait for routing menu commands to focused window.
/// Matches sendMenuCommand in main/ipc.ts which sends "menu-command" via IPC.
pub trait MenuCommandSink: Send + Sync {
    /// Send command string to focused window via IPC.
    fn send_command(&self, command: &str);
}

/// Menu action handlers for special actions that need host-level side effects.
pub trait MenuActionHandlers: Send + Sync {
    /// Check for application updates.
    fn check_for_updates(&self);
    /// Relaunch the application.
    fn relaunch(&self);
}

/// Route a menu item click to the appropriate handler.
pub fn dispatch_menu_click(
    entry: &MenuEntry,
    handlers: &dyn MenuActionHandlers,
    sink: &dyn MenuCommandSink,
) {
    match entry {
        MenuEntry::Item {
            command, action, ..
        } => {
            if let Some(action) = action {
                match action {
                    MenuAction::AppCheckForUpdates => handlers.check_for_updates(),
                    MenuAction::AppRelaunch => handlers.relaunch(),
                    _ => sink.send_command(action.as_command_str()),
                }
            }
            if let Some(command) = command {
                sink.send_command(command);
            }
        }
        MenuEntry::Separator { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_menu_catalog_has_seven_menus() {
        let catalog = build_menu_catalog();
        assert_eq!(catalog.menus.len(), 7);
    }

    #[test]
    fn test_app_menu_macos_only() {
        let catalog = build_menu_catalog();
        let app_menu = catalog.menus.iter().find(|m| m.id == "app").unwrap();
        assert!(app_menu.is_visible_on(Platform::MacOS));
        assert!(!app_menu.is_visible_on(Platform::Windows));
        assert!(!app_menu.is_visible_on(Platform::Linux));
    }

    #[test]
    fn test_file_menu_all_platforms() {
        let catalog = build_menu_catalog();
        let file_menu = catalog.menus.iter().find(|m| m.id == "file").unwrap();
        assert!(file_menu.is_visible_on(Platform::MacOS));
        assert!(file_menu.is_visible_on(Platform::Windows));
        assert!(file_menu.is_visible_on(Platform::Linux));
    }

    #[test]
    fn test_action_command_roundtrip() {
        for action in MenuAction::ALL {
            let cmd = action.as_command_str();
            let parsed = MenuAction::from_command_str(cmd).unwrap();
            assert_eq!(*action, parsed);
        }
    }

    #[test]
    fn test_translation_fallback() {
        let mut messages = HashMap::new();
        messages.insert("desktop.menu.file".to_string(), "Fichier".to_string());

        let bundle = TranslationBundle {
            locale: "fr".to_string(),
            messages,
        };

        let manager = TranslationManager::new(bundle);

        assert_eq!(manager.translate("desktop.menu.file"), "Fichier");
        assert_eq!(manager.translate("desktop.menu.edit"), "Edit");
        assert_eq!(manager.translate("missing.key"), "missing.key");
    }

    #[test]
    fn test_format_message() {
        let mut params = HashMap::new();
        params.insert("version".to_string(), "1.2.3".to_string());
        let result = format_message("Update {{version}} downloaded.", &params);
        assert_eq!(result, "Update 1.2.3 downloaded.");
    }
}
