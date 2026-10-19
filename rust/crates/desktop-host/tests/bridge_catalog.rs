//! Purpose: contract tests for IPC channel catalog round-trips and baseline coverage.
//! Scope: repository transaction.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/bridge/catalog.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: every catalog channel name round-trips; set of names equals the baseline list.
//! Contract tests: this file

use opencode_desktop_host::bridge::catalog::{ChannelKind, IpcChannel};

/// Baseline channel names extracted from packages/desktop/src/preload/index.ts and main/ipc.ts.
/// This is a fixture list to ensure no regressions in channel coverage.
const BASELINE_CHANNELS: &[&str] = &[
    // Invoke
    "kill-sidecar",
    "install-cli",
    "await-initialization",
    "consume-initial-deep-links",
    "get-default-server-url",
    "set-default-server-url",
    "is-first-launch-onboarding-pending",
    "finish-first-launch-onboarding",
    "is-old-layout-eligible",
    "get-display-backend",
    "set-display-backend",
    "check-app-exists",
    "resolve-app-path",
    "updater-subscribe",
    "updater-unsubscribe",
    "updater-check",
    "updater-install",
    "store-get",
    "store-set",
    "store-delete",
    "store-clear",
    "store-keys",
    "store-length",
    "draft-get",
    "draft-set",
    "draft-delete",
    "draft-blob-put",
    "draft-blob-get",
    "get-window-id",
    "open-directory-picker",
    "open-file-picker",
    "read-picked-file",
    "release-picked-files",
    "save-file-picker",
    "open-path",
    "reveal-path",
    "read-clipboard-image",
    "get-window-focused",
    "get-window-fullscreen",
    "set-window-focus",
    "show-window",
    "get-zoom-factor",
    "set-zoom-factor",
    "get-pinch-zoom-enabled",
    "set-pinch-zoom-enabled",
    "set-titlebar",
    "run-desktop-menu-action",
    "set-background-color",
    "export-debug-logs",
    "set-force-focus",
    "record-fatal-renderer-error",
    "set-native-translations",
    // WSL Invoke
    "wsl-servers-get-state",
    "wsl-servers-subscribe",
    "wsl-servers-unsubscribe",
    "wsl-servers-probe-runtime",
    "wsl-servers-refresh-distros",
    "wsl-servers-install-wsl",
    "wsl-servers-install-distro",
    "wsl-servers-probe-addable",
    "wsl-servers-install-opencode",
    "wsl-servers-open-terminal",
    "wsl-servers-add",
    "wsl-servers-remove",
    "wsl-servers-start",
    // Send
    "open-external",
    "open-local-file",
    "relaunch",
    // Event
    "menu-command",
    "deep-link",
    "window-fullscreen-changed",
    "pinch-zoom-enabled-changed",
    "zoom-factor-changed",
    "updater-state",
    "wsl-servers-event",
];

#[test]
fn test_channel_name_round_trip() {
    for channel in IpcChannel::all() {
        if *channel == IpcChannel::Unsupported {
            continue;
        }
        let name = channel.name();
        let parsed = IpcChannel::from_name(name);
        assert_eq!(
            parsed,
            Some(*channel),
            "Round-trip failed for channel: {}",
            name
        );
    }
}

#[test]
fn test_all_channels_cover_baseline() {
    let catalog_names: std::collections::HashSet<&str> = IpcChannel::all()
        .iter()
        .filter(|c| **c != IpcChannel::Unsupported)
        .map(|c| c.name())
        .collect();

    let baseline_set: std::collections::HashSet<&str> = BASELINE_CHANNELS.iter().copied().collect();

    let missing_in_catalog: Vec<&str> = baseline_set.difference(&catalog_names).copied().collect();

    let extra_in_catalog: Vec<&str> = catalog_names.difference(&baseline_set).copied().collect();

    assert!(
        missing_in_catalog.is_empty(),
        "Channels in baseline but missing in catalog: {:?}",
        missing_in_catalog
    );
    assert!(
        extra_in_catalog.is_empty(),
        "Channels in catalog but not in baseline: {:?}",
        extra_in_catalog
    );
}

#[test]
fn test_channel_kinds() {
    assert_eq!(IpcChannel::OpenExternal.kind(), ChannelKind::Send);
    assert_eq!(IpcChannel::OpenLocalFile.kind(), ChannelKind::Send);
    assert_eq!(IpcChannel::Relaunch.kind(), ChannelKind::Send);

    assert_eq!(IpcChannel::MenuCommand.kind(), ChannelKind::Event);
    assert_eq!(IpcChannel::DeepLink.kind(), ChannelKind::Event);
    assert_eq!(
        IpcChannel::WindowFullscreenChanged.kind(),
        ChannelKind::Event
    );
    assert_eq!(
        IpcChannel::PinchZoomEnabledChanged.kind(),
        ChannelKind::Event
    );
    assert_eq!(IpcChannel::ZoomFactorChanged.kind(), ChannelKind::Event);
    assert_eq!(IpcChannel::UpdaterState.kind(), ChannelKind::Event);
    assert_eq!(IpcChannel::WslServersEvent.kind(), ChannelKind::Event);

    assert_eq!(IpcChannel::KillSidecar.kind(), ChannelKind::Invoke);
    assert_eq!(IpcChannel::WslServersGetState.kind(), ChannelKind::Invoke);
}

#[test]
fn test_unsupported_channel() {
    assert_eq!(IpcChannel::Unsupported.name(), "unsupported");
    assert!(IpcChannel::from_name("unknown-channel").is_none());
}
