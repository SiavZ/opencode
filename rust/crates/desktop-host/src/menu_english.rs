//! Purpose: canonical English messages for native desktop capabilities.
//! Scope: menu, updater, recovery, dialog, WSL, and picker fallback text only.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/menu.rs
//! `-- desktop-host/src/menu_translation.rs
//! Invariants: keys, message text, and interpolation placeholders match DESKTOP_NATIVE_ENGLISH.
//! Contract tests: desktop-host/tests/menu_translations.rs

use std::collections::HashMap;

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
