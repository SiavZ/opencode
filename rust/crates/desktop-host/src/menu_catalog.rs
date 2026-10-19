//! Purpose: construct the native desktop menu catalog and its entries.
//! Scope: item builder, platform accelerators, and ordered menu definitions only.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/menu.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/menu.rs
//! Invariants: catalog ordering, roles, commands, accelerators, and platform filters match DESKTOP_MENU.
//! Contract tests: desktop-host/tests/menu_catalog.rs, menu_actions.rs

use super::{
    Accelerators, EnabledCondition, Menu, MenuAction, MenuCatalog, MenuEntry, MenuRole, Platform,
};

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
