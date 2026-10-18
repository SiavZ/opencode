//! Purpose: well-known store names and settings keys shared by the desktop host.
//! Scope: constants only, values match `packages/desktop/src/main/store-keys.ts` and migrate.ts.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/migrate.rs
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/tests/store.rs
//! Invariants: string values are wire/file compatible with the Electron app and never change.
//! Contract tests: desktop-host/tests/store.rs

/// Main settings store file name (`store-keys.ts:1`).
pub const SETTINGS_STORE: &str = "opencode.settings";
pub const DEFAULT_SERVER_URL_KEY: &str = "defaultServerUrl";
pub const FIRST_LAUNCH_ONBOARDING_COMPLETE_KEY: &str = "firstLaunchOnboardingComplete";
pub const OLD_LAYOUT_ELIGIBLE_KEY: &str = "oldLayoutEligible";
pub const WSL_SERVERS_KEY: &str = "wslServers";
pub const PINCH_ZOOM_ENABLED_KEY: &str = "pinchZoomEnabled";
pub const WINDOW_IDS_KEY: &str = "windowIds";
/// Settings flag set once the Tauri-era import ran (`migrate.ts:9`).
pub const TAURI_MIGRATED_KEY: &str = "tauriMigrated";
/// Default renderer storage name when the renderer passes none (`renderer/index.tsx:157`).
pub const DEFAULT_RENDERER_STORE: &str = "default.dat";
/// Global renderer storage name (`packages/app/src/utils/persist.ts:28`).
pub const GLOBAL_RENDERER_STORE: &str = "opencode.global.dat";
