//! Purpose: native menu catalog, translations and desktop menu action routing.
//! Scope: toolkit-independent menu data model, platform-specific layouts, locale translation with English fallback, action routing.
//! Uses:
//! rust/crates/desktop-host/src/
//! `-- lib.rs
//! External:
//! |-- serde::{Serialize, Deserialize}
//! `-- std::collections::HashMap
//! Used by:
//! `-- none, reserved for toolkit adapters (owner: phase-11)
//! Invariants: menu catalog is immutable after construction, translations fall back to English for missing keys, action routing sends commands via sink trait.
//! Contract tests: rust/crates/desktop-host/tests/menu_catalog.rs, rust/crates/desktop-host/tests/menu_translations.rs, rust/crates/desktop-host/tests/menu_actions.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Translation bundle for menu labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationBundle {
    /// Locale identifier.
    pub locale: String,
    /// Translation messages keyed by DesktopNativeKey.
    pub messages: HashMap<String, String>,
}

/// Translation manager with English fallback.
#[derive(Debug, Clone)]
pub struct TranslationManager {
    /// Current bundle.
    bundle: TranslationBundle,
    /// English fallback messages.
    english: HashMap<String, String>,
}

impl TranslationManager {
    /// Create new translation manager with English fallback.
    pub fn new(bundle: TranslationBundle) -> Self {
        let english = english_messages();
        Self { bundle, english }
    }

    /// Update translation bundle. Returns true if bundle changed.
    pub fn set_bundle(&mut self, bundle: TranslationBundle) -> bool {
        if bundle.locale == self.bundle.locale && bundle.messages == self.bundle.messages {
            return false;
        }
        self.bundle = bundle;
        true
    }

    /// Translate a key with English fallback for missing keys.
    pub fn translate(&self, key: &str) -> String {
        self.bundle
            .messages
            .get(key)
            .or_else(|| self.english.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Translate a key with parameter substitution and English fallback.
    pub fn translate_with_params(&self, key: &str, params: &HashMap<String, String>) -> String {
        let template = self.translate(key);
        format_message(&template, params)
    }

    /// Get current locale.
    pub fn locale(&self) -> &str {
        &self.bundle.locale
    }

    /// Get the current bundle.
    pub fn bundle(&self) -> &TranslationBundle {
        &self.bundle
    }
}

/// Format a message template by replacing `{{key}}` placeholders.
/// Matches TypeScript formatDesktopNativeMessage behavior.
pub fn format_message(template: &str, params: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in params {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
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

/// Builder for creating MenuEntry::Item.
#[derive(Default)]
pub struct MenuItemBuilder {
    label_key: Option<String>,
    command: Option<String>,
    action: Option<MenuAction>,
    role: Option<MenuRole>,
    href: Option<String>,
    accelerator: Option<Accelerators>,
    enabled: Option<EnabledCondition>,
    platforms: Option<Vec<Platform>>,
}

impl MenuItemBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn label_key(mut self, key: &str) -> Self {
        self.label_key = Some(key.to_string());
        self
    }

    pub fn command(mut self, cmd: &str) -> Self {
        self.command = Some(cmd.to_string());
        self
    }

    pub fn action(mut self, action: MenuAction) -> Self {
        self.action = Some(action);
        self
    }

    pub fn role(mut self, role: MenuRole) -> Self {
        self.role = Some(role);
        self
    }

    pub fn href(mut self, url: &str) -> Self {
        self.href = Some(url.to_string());
        self
    }

    pub fn accelerator(mut self, acc: Accelerators) -> Self {
        self.accelerator = Some(acc);
        self
    }

    pub fn enabled(mut self, cond: EnabledCondition) -> Self {
        self.enabled = Some(cond);
        self
    }

    pub fn platforms(mut self, platforms: Vec<Platform>) -> Self {
        self.platforms = Some(platforms);
        self
    }

    pub fn build(self) -> MenuEntry {
        MenuEntry::Item {
            label_key: self.label_key,
            command: self.command,
            action: self.action,
            role: self.role,
            href: self.href,
            accelerator: self.accelerator,
            enabled: self.enabled,
            platforms: self.platforms,
        }
    }
}

fn role_item(role: MenuRole) -> MenuEntry {
    MenuItemBuilder::new().role(role).build()
}

fn sep() -> MenuEntry {
    MenuEntry::Separator { platforms: None }
}

fn macos_accel(s: &str) -> Option<Accelerators> {
    Some(Accelerators {
        macos: Some(s.to_string()),
        windows: None,
        linux: None,
    })
}

fn windows_accel(s: &str) -> Option<Accelerators> {
    Some(Accelerators {
        macos: None,
        windows: Some(s.to_string()),
        linux: None,
    })
}

fn dual_accel(macos: &str, windows: &str) -> Option<Accelerators> {
    Some(Accelerators {
        macos: Some(macos.to_string()),
        windows: Some(windows.to_string()),
        linux: None,
    })
}

fn app_menu() -> Menu {
    Menu {
        id: "app".to_string(),
        label_key: "desktop.menu.app".to_string(),
        role: None,
        items: Some(vec![
            role_item(MenuRole::About),
            MenuItemBuilder::new()
                .label_key("desktop.menu.checkForUpdates")
                .action(MenuAction::AppCheckForUpdates)
                .enabled(EnabledCondition::Updater)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.settings")
                .command("settings.open")
                .accelerator(macos_accel("Cmd+,").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.reloadWebview")
                .action(MenuAction::ViewReload)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.restart")
                .action(MenuAction::AppRelaunch)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.exportLogs")
                .command("logs.export")
                .build(),
            sep(),
            role_item(MenuRole::Hide),
            role_item(MenuRole::HideOthers),
            role_item(MenuRole::Unhide),
            sep(),
            role_item(MenuRole::Quit),
        ]),
        platforms: Some(vec![Platform::MacOS]),
    }
}

fn file_menu() -> Menu {
    Menu {
        id: "file".to_string(),
        label_key: "desktop.menu.file".to_string(),
        role: None,
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.newSession")
                .command("session.new")
                .accelerator(macos_accel("Shift+Cmd+S").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.openProject")
                .command("project.open")
                .accelerator(macos_accel("Cmd+O").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.settings")
                .command("settings.open")
                .accelerator(windows_accel("Ctrl+,").unwrap())
                .platforms(vec![Platform::Windows])
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.newWindow")
                .action(MenuAction::WindowNew)
                .accelerator(dual_accel("Cmd+Shift+N", "Ctrl+Shift+N").unwrap())
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.closeWindow")
                .action(MenuAction::WindowClose)
                .role(MenuRole::Close)
                .build(),
        ]),
        platforms: None,
    }
}

fn edit_menu() -> Menu {
    Menu {
        id: "edit".to_string(),
        label_key: "desktop.menu.edit".to_string(),
        role: None,
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.undo")
                .action(MenuAction::EditUndo)
                .role(MenuRole::Undo)
                .accelerator(windows_accel("Ctrl+Z").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.redo")
                .action(MenuAction::EditRedo)
                .role(MenuRole::Redo)
                .accelerator(windows_accel("Ctrl+Y").unwrap())
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.cut")
                .action(MenuAction::EditCut)
                .role(MenuRole::Cut)
                .accelerator(windows_accel("Ctrl+X").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.copy")
                .action(MenuAction::EditCopy)
                .role(MenuRole::Copy)
                .accelerator(windows_accel("Ctrl+C").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.paste")
                .action(MenuAction::EditPaste)
                .role(MenuRole::Paste)
                .accelerator(windows_accel("Ctrl+V").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.delete")
                .action(MenuAction::EditDelete)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.selectAll")
                .action(MenuAction::EditSelectAll)
                .role(MenuRole::SelectAll)
                .accelerator(windows_accel("Ctrl+A").unwrap())
                .build(),
        ]),
        platforms: None,
    }
}

fn view_menu() -> Menu {
    Menu {
        id: "view".to_string(),
        label_key: "desktop.menu.view".to_string(),
        role: None,
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.toggleSidebar")
                .command("sidebar.toggle")
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.toggleTerminal")
                .command("terminal.toggle")
                .accelerator(macos_accel("Ctrl+`").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.toggleFileTree")
                .command("fileTree.toggle")
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.reload")
                .action(MenuAction::ViewReload)
                .role(MenuRole::Reload)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.toggleDeveloperTools")
                .action(MenuAction::ViewToggleDevTools)
                .role(MenuRole::ToggleDevTools)
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.actualSize")
                .action(MenuAction::ViewResetZoom)
                .role(MenuRole::ResetZoom)
                .accelerator(windows_accel("Ctrl+0").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.zoomIn")
                .action(MenuAction::ViewZoomIn)
                .role(MenuRole::ZoomIn)
                .accelerator(windows_accel("Ctrl++").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.zoomOut")
                .action(MenuAction::ViewZoomOut)
                .role(MenuRole::ZoomOut)
                .accelerator(windows_accel("Ctrl+-").unwrap())
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.toggleFullScreen")
                .action(MenuAction::ViewToggleFullscreen)
                .role(MenuRole::ToggleFullscreen)
                .build(),
        ]),
        platforms: None,
    }
}

fn go_menu() -> Menu {
    Menu {
        id: "go".to_string(),
        label_key: "desktop.menu.go".to_string(),
        role: None,
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.back")
                .command("common.goBack")
                .accelerator(macos_accel("Cmd+[").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.forward")
                .command("common.goForward")
                .accelerator(macos_accel("Cmd+]").unwrap())
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.previousSession")
                .command("session.previous")
                .accelerator(macos_accel("Option+Up").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.nextSession")
                .command("session.next")
                .accelerator(macos_accel("Option+Down").unwrap())
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.previousProject")
                .command("project.previous")
                .accelerator(macos_accel("Cmd+Option+Up").unwrap())
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.nextProject")
                .command("project.next")
                .accelerator(macos_accel("Cmd+Option+Down").unwrap())
                .build(),
        ]),
        platforms: None,
    }
}

fn window_menu() -> Menu {
    Menu {
        id: "window".to_string(),
        label_key: "desktop.menu.window".to_string(),
        role: Some(MenuRole::WindowMenu),
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.minimize")
                .action(MenuAction::WindowMinimize)
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.maximize")
                .action(MenuAction::WindowToggleMaximize)
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.closeWindow")
                .action(MenuAction::WindowClose)
                .build(),
        ]),
        platforms: None,
    }
}

fn help_menu() -> Menu {
    Menu {
        id: "help".to_string(),
        label_key: "desktop.menu.help".to_string(),
        role: None,
        items: Some(vec![
            MenuItemBuilder::new()
                .label_key("desktop.menu.documentation")
                .href("https://opencode.ai/docs")
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.supportForum")
                .href("https://discord.com/invite/opencode")
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.exportLogs")
                .command("logs.export")
                .build(),
            sep(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.shareFeedback")
                .href(
                    "https://github.com/anomalyco/opencode/issues/new?template=feature_request.yml",
                )
                .build(),
            MenuItemBuilder::new()
                .label_key("desktop.menu.reportBug")
                .href("https://github.com/anomalyco/opencode/issues/new?template=bug_report.yml")
                .build(),
        ]),
        platforms: None,
    }
}

/// Build the complete menu catalog matching TypeScript DESKTOP_MENU.
pub fn build_menu_catalog() -> MenuCatalog {
    MenuCatalog {
        menus: vec![
            app_menu(),
            file_menu(),
            edit_menu(),
            view_menu(),
            go_menu(),
            window_menu(),
            help_menu(),
        ],
    }
}

/// English translation messages matching TypeScript DESKTOP_NATIVE_ENGLISH.
pub fn english_messages() -> HashMap<String, String> {
    let mut m = HashMap::new();

    // Menu labels
    m.insert("desktop.menu.app".into(), "OpenCode".into());
    m.insert("desktop.menu.file".into(), "File".into());
    m.insert("desktop.menu.edit".into(), "Edit".into());
    m.insert("desktop.menu.view".into(), "View".into());
    m.insert("desktop.menu.go".into(), "Go".into());
    m.insert("desktop.menu.window".into(), "Window".into());
    m.insert("desktop.menu.help".into(), "Help".into());
    m.insert(
        "desktop.menu.checkForUpdates".into(),
        "Check for Updates...".into(),
    );
    m.insert("desktop.menu.settings".into(), "Settings".into());
    m.insert("desktop.menu.reloadWebview".into(), "Reload Webview".into());
    m.insert("desktop.menu.restart".into(), "Restart".into());
    m.insert("desktop.menu.exportLogs".into(), "Export Logs...".into());
    m.insert("desktop.menu.newSession".into(), "New Session".into());
    m.insert("desktop.menu.openProject".into(), "Open Project...".into());
    m.insert("desktop.menu.newWindow".into(), "New Window".into());
    m.insert("desktop.menu.closeWindow".into(), "Close Window".into());
    m.insert("desktop.menu.undo".into(), "Undo".into());
    m.insert("desktop.menu.redo".into(), "Redo".into());
    m.insert("desktop.menu.cut".into(), "Cut".into());
    m.insert("desktop.menu.copy".into(), "Copy".into());
    m.insert("desktop.menu.paste".into(), "Paste".into());
    m.insert("desktop.menu.delete".into(), "Delete".into());
    m.insert("desktop.menu.selectAll".into(), "Select All".into());
    m.insert("desktop.menu.toggleSidebar".into(), "Toggle Sidebar".into());
    m.insert(
        "desktop.menu.toggleTerminal".into(),
        "Toggle Terminal".into(),
    );
    m.insert(
        "desktop.menu.toggleFileTree".into(),
        "Toggle File Tree".into(),
    );
    m.insert("desktop.menu.reload".into(), "Reload".into());
    m.insert(
        "desktop.menu.toggleDeveloperTools".into(),
        "Toggle Developer Tools".into(),
    );
    m.insert("desktop.menu.actualSize".into(), "Actual Size".into());
    m.insert("desktop.menu.zoomIn".into(), "Zoom In".into());
    m.insert("desktop.menu.zoomOut".into(), "Zoom Out".into());
    m.insert(
        "desktop.menu.toggleFullScreen".into(),
        "Toggle Full Screen".into(),
    );
    m.insert("desktop.menu.back".into(), "Back".into());
    m.insert("desktop.menu.forward".into(), "Forward".into());
    m.insert(
        "desktop.menu.previousSession".into(),
        "Previous Session".into(),
    );
    m.insert("desktop.menu.nextSession".into(), "Next Session".into());
    m.insert(
        "desktop.menu.previousProject".into(),
        "Previous Project".into(),
    );
    m.insert("desktop.menu.nextProject".into(), "Next Project".into());
    m.insert("desktop.menu.minimize".into(), "Minimize".into());
    m.insert("desktop.menu.maximize".into(), "Maximize".into());
    m.insert(
        "desktop.menu.documentation".into(),
        "OpenCode Documentation".into(),
    );
    m.insert("desktop.menu.supportForum".into(), "Support Forum".into());
    m.insert("desktop.menu.shareFeedback".into(), "Share Feedback".into());
    m.insert("desktop.menu.reportBug".into(), "Report a Bug".into());
    m.insert("desktop.menu.ariaLabel".into(), "OpenCode menu".into());

    // Updater dialogs
    m.insert(
        "desktop.updater.dialog.checkFailed.message".into(),
        "Update check failed.".into(),
    );
    m.insert(
        "desktop.updater.dialog.checkFailed.title".into(),
        "Update Error".into(),
    );
    m.insert(
        "desktop.updater.dialog.upToDate.message".into(),
        "You're up to date.".into(),
    );
    m.insert(
        "desktop.updater.dialog.upToDate.title".into(),
        "No Updates".into(),
    );
    m.insert(
        "desktop.updater.dialog.ready.message".into(),
        "Update {{version}} downloaded. Restart now?".into(),
    );
    m.insert(
        "desktop.updater.dialog.ready.title".into(),
        "Update Ready".into(),
    );
    m.insert("desktop.updater.dialog.restart".into(), "Restart".into());
    m.insert("desktop.updater.dialog.later".into(), "Later".into());

    // Recovery actions
    m.insert("desktop.recovery.action.relaunch".into(), "Relaunch".into());
    m.insert(
        "desktop.recovery.action.exportLogs".into(),
        "Export Logs".into(),
    );
    m.insert(
        "desktop.recovery.action.keepWaiting".into(),
        "Keep Waiting".into(),
    );
    m.insert("desktop.recovery.action.quit".into(), "Quit".into());
    m.insert(
        "desktop.recovery.loadFailed".into(),
        "OpenCode failed to load".into(),
    );
    m.insert(
        "desktop.recovery.terminated".into(),
        "OpenCode window terminated unexpectedly".into(),
    );
    m.insert(
        "desktop.recovery.unresponsive".into(),
        "OpenCode is not responding".into(),
    );
    m.insert(
        "desktop.recovery.unresponsive.detail".into(),
        "You can relaunch the app, open the logs, or keep waiting.".into(),
    );
    m.insert(
        "desktop.recovery.loadFailed.detail".into(),
        "Window: {{window}}\nURL: {{url}}\nError: {{code}} {{description}}".into(),
    );
    m.insert(
        "desktop.recovery.terminated.detail".into(),
        "Window: {{window}}\nReason: {{reason}}\nCode: {{code}}".into(),
    );
    m.insert("desktop.recovery.unknown".into(), "<unknown>".into());

    // Dialogs
    m.insert(
        "desktop.dialog.chooseFolder".into(),
        "Choose a folder".into(),
    );
    m.insert("desktop.dialog.chooseFile".into(), "Choose a file".into());
    m.insert("desktop.dialog.saveFile".into(), "Save file".into());
    m.insert("desktop.dialog.files".into(), "Files".into());
    m.insert("desktop.server.local".into(), "Local Server".into());

    // WSL errors
    m.insert(
        "desktop.wsl.error.windowsOnly".into(),
        "WSL is only available on Windows".into(),
    );
    m.insert(
        "desktop.wsl.error.unavailable".into(),
        "WSL is unavailable".into(),
    );
    m.insert(
        "desktop.wsl.error.listInstalled".into(),
        "Failed to list installed WSL distros".into(),
    );
    m.insert(
        "desktop.wsl.error.listOnline".into(),
        "Failed to list online WSL distros".into(),
    );
    m.insert(
        "desktop.wsl.error.executeDistro".into(),
        "Cannot execute commands in distro".into(),
    );
    m.insert(
        "desktop.wsl.error.installWsl".into(),
        "WSL installation failed".into(),
    );
    m.insert(
        "desktop.wsl.error.installDistro".into(),
        "Failed to install distro: {{distro}}".into(),
    );
    m.insert(
        "desktop.wsl.error.installOpencode".into(),
        "OpenCode installation failed".into(),
    );
    m.insert(
        "desktop.wsl.error.alreadyAdded".into(),
        "{{distro}} is already added".into(),
    );
    m.insert(
        "desktop.wsl.error.opencodeMissing".into(),
        "opencode is not installed in this distro".into(),
    );
    m.insert(
        "desktop.wsl.error.opencodeCannotRun".into(),
        "opencode is installed but could not run".into(),
    );
    m.insert(
        "desktop.wsl.error.opencodeNotInstalled".into(),
        "OpenCode is not installed in {{distro}}".into(),
    );
    m.insert("desktop.wsl.error.updateVersion".into(), "OpenCode update finished but {{distro}} still reports {{installed}}; expected {{expected}}".into());
    m.insert("desktop.wsl.error.noVersion".into(), "no version".into());
    m.insert(
        "desktop.wsl.error.serverExited".into(),
        "WSL server exited after startup (code={{code}} signal={{signal}})".into(),
    );
    m.insert(
        "desktop.wsl.error.serverExitedBeforeHealthy".into(),
        "WSL server exited before becoming healthy (code={{code}} signal={{signal}}){{output}}"
            .into(),
    );
    m.insert(
        "desktop.wsl.error.healthTimeout".into(),
        "Sidecar for {{distro}} health check timed out after {{timeout}}ms".into(),
    );
    m.insert(
        "desktop.wsl.error.commandTimeout".into(),
        "{{command}} {{args}} timed out after {{timeout}}ms".into(),
    );
    m.insert(
        "desktop.wsl.error.failedPort".into(),
        "Failed to get port".into(),
    );

    // Picker errors
    m.insert(
        "desktop.picker.error.notSelected".into(),
        "File was not selected by the picker".into(),
    );
    m.insert(
        "desktop.picker.error.sizeLimit".into(),
        "Selected attachments exceed the {{limit}} MB limit".into(),
    );

    m
}

#[cfg(test)]
mod tests {
    use super::*;

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
