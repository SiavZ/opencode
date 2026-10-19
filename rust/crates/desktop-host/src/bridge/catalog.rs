//! Purpose: typed catalog of every desktop IPC invoke and event channel with exact wire names and payloads.
//! Scope: repository transaction.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/bridge.rs
//! |-- desktop-host/src/bridge/dispatch.rs
//! |-- desktop-host/tests/bridge_catalog.rs
//! `-- desktop-host/tests/bridge_dispatch.rs
//! Invariants: wire channel names match packages/desktop/src/preload and main/ipc.ts exactly.
//! Contract tests: desktop-host/tests/bridge_catalog.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Direction of the IPC channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    /// Renderer invokes, main handles (request-response).
    Invoke,
    /// Renderer sends, main receives (fire-and-forget).
    Send,
    /// Main sends, renderer receives (event).
    Event,
}

/// Typed catalog of every IPC channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpcChannel {
    // --- Invoke Channels ---
    KillSidecar,
    InstallCli,
    AwaitInitialization,
    ConsumeInitialDeepLinks,
    GetDefaultServerUrl,
    SetDefaultServerUrl,
    IsFirstLaunchOnboardingPending,
    FinishFirstLaunchOnboarding,
    IsOldLayoutEligible,
    GetDisplayBackend,
    SetDisplayBackend,
    CheckAppExists,
    ResolveAppPath,
    UpdaterSubscribe,
    UpdaterUnsubscribe,
    UpdaterCheck,
    UpdaterInstall,
    StoreGet,
    StoreSet,
    StoreDelete,
    StoreClear,
    StoreKeys,
    StoreLength,
    DraftGet,
    DraftSet,
    DraftDelete,
    DraftBlobPut,
    DraftBlobGet,
    GetWindowId,
    OpenDirectoryPicker,
    OpenFilePicker,
    ReadPickedFile,
    ReleasePickedFiles,
    SaveFilePicker,
    OpenPath,
    RevealPath,
    ReadClipboardImage,
    GetWindowFocused,
    GetWindowFullscreen,
    SetWindowFocus,
    ShowWindow,
    GetZoomFactor,
    SetZoomFactor,
    GetPinchZoomEnabled,
    SetPinchZoomEnabled,
    SetTitlebar,
    RunDesktopMenuAction,
    SetBackgroundColor,
    ExportDebugLogs,
    SetForceFocus,
    RecordFatalRendererError,
    SetNativeTranslations,
    // WSL Invoke Channels
    WslServersGetState,
    WslServersSubscribe,
    WslServersUnsubscribe,
    WslServersProbeRuntime,
    WslServersRefreshDistros,
    WslServersInstallWsl,
    WslServersInstallDistro,
    WslServersProbeAddable,
    WslServersInstallOpencode,
    WslServersOpenTerminal,
    WslServersAdd,
    WslServersRemove,
    WslServersStart,

    // --- Send Channels (Renderer -> Main) ---
    OpenExternal,
    OpenLocalFile,
    Relaunch,

    // --- Event Channels (Main -> Renderer) ---
    MenuCommand,
    DeepLink,
    WindowFullscreenChanged,
    PinchZoomEnabledChanged,
    ZoomFactorChanged,
    UpdaterState,
    WslServersEvent,

    // --- Discrepancies / Unsupported ---
    /// Exposed in preload but has no handler in main, or vice versa.
    #[serde(rename = "unsupported")]
    Unsupported,
}

impl IpcChannel {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::KillSidecar => "kill-sidecar",
            Self::InstallCli => "install-cli",
            Self::AwaitInitialization => "await-initialization",
            Self::ConsumeInitialDeepLinks => "consume-initial-deep-links",
            Self::GetDefaultServerUrl => "get-default-server-url",
            Self::SetDefaultServerUrl => "set-default-server-url",
            Self::IsFirstLaunchOnboardingPending => "is-first-launch-onboarding-pending",
            Self::FinishFirstLaunchOnboarding => "finish-first-launch-onboarding",
            Self::IsOldLayoutEligible => "is-old-layout-eligible",
            Self::GetDisplayBackend => "get-display-backend",
            Self::SetDisplayBackend => "set-display-backend",
            Self::CheckAppExists => "check-app-exists",
            Self::ResolveAppPath => "resolve-app-path",
            Self::UpdaterSubscribe => "updater-subscribe",
            Self::UpdaterUnsubscribe => "updater-unsubscribe",
            Self::UpdaterCheck => "updater-check",
            Self::UpdaterInstall => "updater-install",
            Self::StoreGet => "store-get",
            Self::StoreSet => "store-set",
            Self::StoreDelete => "store-delete",
            Self::StoreClear => "store-clear",
            Self::StoreKeys => "store-keys",
            Self::StoreLength => "store-length",
            Self::DraftGet => "draft-get",
            Self::DraftSet => "draft-set",
            Self::DraftDelete => "draft-delete",
            Self::DraftBlobPut => "draft-blob-put",
            Self::DraftBlobGet => "draft-blob-get",
            Self::GetWindowId => "get-window-id",
            Self::OpenDirectoryPicker => "open-directory-picker",
            Self::OpenFilePicker => "open-file-picker",
            Self::ReadPickedFile => "read-picked-file",
            Self::ReleasePickedFiles => "release-picked-files",
            Self::SaveFilePicker => "save-file-picker",
            Self::OpenPath => "open-path",
            Self::RevealPath => "reveal-path",
            Self::ReadClipboardImage => "read-clipboard-image",
            Self::GetWindowFocused => "get-window-focused",
            Self::GetWindowFullscreen => "get-window-fullscreen",
            Self::SetWindowFocus => "set-window-focus",
            Self::ShowWindow => "show-window",
            Self::GetZoomFactor => "get-zoom-factor",
            Self::SetZoomFactor => "set-zoom-factor",
            Self::GetPinchZoomEnabled => "get-pinch-zoom-enabled",
            Self::SetPinchZoomEnabled => "set-pinch-zoom-enabled",
            Self::SetTitlebar => "set-titlebar",
            Self::RunDesktopMenuAction => "run-desktop-menu-action",
            Self::SetBackgroundColor => "set-background-color",
            Self::ExportDebugLogs => "export-debug-logs",
            Self::SetForceFocus => "set-force-focus",
            Self::RecordFatalRendererError => "record-fatal-renderer-error",
            Self::SetNativeTranslations => "set-native-translations",
            Self::WslServersGetState => "wsl-servers-get-state",
            Self::WslServersSubscribe => "wsl-servers-subscribe",
            Self::WslServersUnsubscribe => "wsl-servers-unsubscribe",
            Self::WslServersProbeRuntime => "wsl-servers-probe-runtime",
            Self::WslServersRefreshDistros => "wsl-servers-refresh-distros",
            Self::WslServersInstallWsl => "wsl-servers-install-wsl",
            Self::WslServersInstallDistro => "wsl-servers-install-distro",
            Self::WslServersProbeAddable => "wsl-servers-probe-addable",
            Self::WslServersInstallOpencode => "wsl-servers-install-opencode",
            Self::WslServersOpenTerminal => "wsl-servers-open-terminal",
            Self::WslServersAdd => "wsl-servers-add",
            Self::WslServersRemove => "wsl-servers-remove",
            Self::WslServersStart => "wsl-servers-start",
            Self::OpenExternal => "open-external",
            Self::OpenLocalFile => "open-local-file",
            Self::Relaunch => "relaunch",
            Self::MenuCommand => "menu-command",
            Self::DeepLink => "deep-link",
            Self::WindowFullscreenChanged => "window-fullscreen-changed",
            Self::PinchZoomEnabledChanged => "pinch-zoom-enabled-changed",
            Self::ZoomFactorChanged => "zoom-factor-changed",
            Self::UpdaterState => "updater-state",
            Self::WslServersEvent => "wsl-servers-event",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "kill-sidecar" => Some(Self::KillSidecar),
            "install-cli" => Some(Self::InstallCli),
            "await-initialization" => Some(Self::AwaitInitialization),
            "consume-initial-deep-links" => Some(Self::ConsumeInitialDeepLinks),
            "get-default-server-url" => Some(Self::GetDefaultServerUrl),
            "set-default-server-url" => Some(Self::SetDefaultServerUrl),
            "is-first-launch-onboarding-pending" => Some(Self::IsFirstLaunchOnboardingPending),
            "finish-first-launch-onboarding" => Some(Self::FinishFirstLaunchOnboarding),
            "is-old-layout-eligible" => Some(Self::IsOldLayoutEligible),
            "get-display-backend" => Some(Self::GetDisplayBackend),
            "set-display-backend" => Some(Self::SetDisplayBackend),
            "check-app-exists" => Some(Self::CheckAppExists),
            "resolve-app-path" => Some(Self::ResolveAppPath),
            "updater-subscribe" => Some(Self::UpdaterSubscribe),
            "updater-unsubscribe" => Some(Self::UpdaterUnsubscribe),
            "updater-check" => Some(Self::UpdaterCheck),
            "updater-install" => Some(Self::UpdaterInstall),
            "store-get" => Some(Self::StoreGet),
            "store-set" => Some(Self::StoreSet),
            "store-delete" => Some(Self::StoreDelete),
            "store-clear" => Some(Self::StoreClear),
            "store-keys" => Some(Self::StoreKeys),
            "store-length" => Some(Self::StoreLength),
            "draft-get" => Some(Self::DraftGet),
            "draft-set" => Some(Self::DraftSet),
            "draft-delete" => Some(Self::DraftDelete),
            "draft-blob-put" => Some(Self::DraftBlobPut),
            "draft-blob-get" => Some(Self::DraftBlobGet),
            "get-window-id" => Some(Self::GetWindowId),
            "open-directory-picker" => Some(Self::OpenDirectoryPicker),
            "open-file-picker" => Some(Self::OpenFilePicker),
            "read-picked-file" => Some(Self::ReadPickedFile),
            "release-picked-files" => Some(Self::ReleasePickedFiles),
            "save-file-picker" => Some(Self::SaveFilePicker),
            "open-path" => Some(Self::OpenPath),
            "reveal-path" => Some(Self::RevealPath),
            "read-clipboard-image" => Some(Self::ReadClipboardImage),
            "get-window-focused" => Some(Self::GetWindowFocused),
            "get-window-fullscreen" => Some(Self::GetWindowFullscreen),
            "set-window-focus" => Some(Self::SetWindowFocus),
            "show-window" => Some(Self::ShowWindow),
            "get-zoom-factor" => Some(Self::GetZoomFactor),
            "set-zoom-factor" => Some(Self::SetZoomFactor),
            "get-pinch-zoom-enabled" => Some(Self::GetPinchZoomEnabled),
            "set-pinch-zoom-enabled" => Some(Self::SetPinchZoomEnabled),
            "set-titlebar" => Some(Self::SetTitlebar),
            "run-desktop-menu-action" => Some(Self::RunDesktopMenuAction),
            "set-background-color" => Some(Self::SetBackgroundColor),
            "export-debug-logs" => Some(Self::ExportDebugLogs),
            "set-force-focus" => Some(Self::SetForceFocus),
            "record-fatal-renderer-error" => Some(Self::RecordFatalRendererError),
            "set-native-translations" => Some(Self::SetNativeTranslations),
            "wsl-servers-get-state" => Some(Self::WslServersGetState),
            "wsl-servers-subscribe" => Some(Self::WslServersSubscribe),
            "wsl-servers-unsubscribe" => Some(Self::WslServersUnsubscribe),
            "wsl-servers-probe-runtime" => Some(Self::WslServersProbeRuntime),
            "wsl-servers-refresh-distros" => Some(Self::WslServersRefreshDistros),
            "wsl-servers-install-wsl" => Some(Self::WslServersInstallWsl),
            "wsl-servers-install-distro" => Some(Self::WslServersInstallDistro),
            "wsl-servers-probe-addable" => Some(Self::WslServersProbeAddable),
            "wsl-servers-install-opencode" => Some(Self::WslServersInstallOpencode),
            "wsl-servers-open-terminal" => Some(Self::WslServersOpenTerminal),
            "wsl-servers-add" => Some(Self::WslServersAdd),
            "wsl-servers-remove" => Some(Self::WslServersRemove),
            "wsl-servers-start" => Some(Self::WslServersStart),
            "open-external" => Some(Self::OpenExternal),
            "open-local-file" => Some(Self::OpenLocalFile),
            "relaunch" => Some(Self::Relaunch),
            "menu-command" => Some(Self::MenuCommand),
            "deep-link" => Some(Self::DeepLink),
            "window-fullscreen-changed" => Some(Self::WindowFullscreenChanged),
            "pinch-zoom-enabled-changed" => Some(Self::PinchZoomEnabledChanged),
            "zoom-factor-changed" => Some(Self::ZoomFactorChanged),
            "updater-state" => Some(Self::UpdaterState),
            "wsl-servers-event" => Some(Self::WslServersEvent),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::KillSidecar,
            Self::InstallCli,
            Self::AwaitInitialization,
            Self::ConsumeInitialDeepLinks,
            Self::GetDefaultServerUrl,
            Self::SetDefaultServerUrl,
            Self::IsFirstLaunchOnboardingPending,
            Self::FinishFirstLaunchOnboarding,
            Self::IsOldLayoutEligible,
            Self::GetDisplayBackend,
            Self::SetDisplayBackend,
            Self::CheckAppExists,
            Self::ResolveAppPath,
            Self::UpdaterSubscribe,
            Self::UpdaterUnsubscribe,
            Self::UpdaterCheck,
            Self::UpdaterInstall,
            Self::StoreGet,
            Self::StoreSet,
            Self::StoreDelete,
            Self::StoreClear,
            Self::StoreKeys,
            Self::StoreLength,
            Self::DraftGet,
            Self::DraftSet,
            Self::DraftDelete,
            Self::DraftBlobPut,
            Self::DraftBlobGet,
            Self::GetWindowId,
            Self::OpenDirectoryPicker,
            Self::OpenFilePicker,
            Self::ReadPickedFile,
            Self::ReleasePickedFiles,
            Self::SaveFilePicker,
            Self::OpenPath,
            Self::RevealPath,
            Self::ReadClipboardImage,
            Self::GetWindowFocused,
            Self::GetWindowFullscreen,
            Self::SetWindowFocus,
            Self::ShowWindow,
            Self::GetZoomFactor,
            Self::SetZoomFactor,
            Self::GetPinchZoomEnabled,
            Self::SetPinchZoomEnabled,
            Self::SetTitlebar,
            Self::RunDesktopMenuAction,
            Self::SetBackgroundColor,
            Self::ExportDebugLogs,
            Self::SetForceFocus,
            Self::RecordFatalRendererError,
            Self::SetNativeTranslations,
            Self::WslServersGetState,
            Self::WslServersSubscribe,
            Self::WslServersUnsubscribe,
            Self::WslServersProbeRuntime,
            Self::WslServersRefreshDistros,
            Self::WslServersInstallWsl,
            Self::WslServersInstallDistro,
            Self::WslServersProbeAddable,
            Self::WslServersInstallOpencode,
            Self::WslServersOpenTerminal,
            Self::WslServersAdd,
            Self::WslServersRemove,
            Self::WslServersStart,
            Self::OpenExternal,
            Self::OpenLocalFile,
            Self::Relaunch,
            Self::MenuCommand,
            Self::DeepLink,
            Self::WindowFullscreenChanged,
            Self::PinchZoomEnabledChanged,
            Self::ZoomFactorChanged,
            Self::UpdaterState,
            Self::WslServersEvent,
            Self::Unsupported,
        ]
    }

    pub const fn kind(&self) -> ChannelKind {
        match self {
            Self::OpenExternal | Self::OpenLocalFile | Self::Relaunch => ChannelKind::Send,
            Self::MenuCommand
            | Self::DeepLink
            | Self::WindowFullscreenChanged
            | Self::PinchZoomEnabledChanged
            | Self::ZoomFactorChanged
            | Self::UpdaterState
            | Self::WslServersEvent => ChannelKind::Event,
            _ => ChannelKind::Invoke,
        }
    }
}

/// Common payload types for IPC channels.
/// Complex or binary payloads use `serde_json::Value` to avoid dependency bloat
/// while preserving exact field names and optionality in structured variants.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerReadyData {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitlebarTheme {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FatalRendererError {
    pub error: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenDirectoryPickerOpts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFilePickerOpts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFilePickerResult {
    pub token: String,
    pub files: Vec<PickedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFilePickerOpts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenPathOpts {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardImage {
    pub buffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopMenuActionPayload {
    pub action: String,
    #[serde(flatten)]
    pub extra: Value,
}
