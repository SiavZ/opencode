# Windows Policy (Navigation, Recovery, Unresponsive)

## Inventory Rows
- `packages/desktop/src/main/external-url.ts`: `resolveExternalURL`, `resolveLocalFilePath`
- `packages/desktop/src/main/external-url.test.ts`: 4 test cases for URL resolution
- `packages/desktop/src/main/windows.ts`: `wireNavigationPolicy`, `wireWindowRecovery`, `allowRendererPermissions`, `isTrustedRendererUrl`
- `packages/desktop/src/main/unresponsive.ts`: `createUnresponsiveSampler` (1000ms interval, 15000ms period)
- `packages/desktop/src/main/index.ts:234-251`: App-level `render-process-gone` logging

## Contract
- Pure, time-injected policy state machine in `rust/crates/desktop-host/src/windows/policy.rs`.
- `resolve_external_url`: Matches TS exactly (http, https, mailto allowed; file, javascript, data, invalid denied).
- `resolve_local_file_path`: Matches TS exactly (file protocol, no hostname, valid file path).
- `is_trusted_renderer_url`: Checks `oc://renderer` or dev server origin, with optional `.html` suffix requirement.
- `evaluate_navigation`: Returns `AllowInWindow` or `DenyAndOpenExternal`.
- `evaluate_permission`: Validates `clipboard-sanitized-write` or `notifications` against trusted origin and matching web contents ID.
- `UnresponsiveSampler`: Pure state machine tracking `Idle`, `Sampling`, `ShowingDialog`. Supports `start`, `collect_sample`, `stop_and_flush`, `check_timeout`.
- `evaluate_recovery_event`: Maps `RenderProcessGone`, `Unresponsive`, `Responsive`, `LoadFailed` to `RecoveryResult` (e.g., `ShowDialog`, `StopSamplingAndFlush`, `NoAction` for error code -3).
- `evaluate_recovery_action`: Maps user choices (`Relaunch`, `ExportLogs`, `KeepWaiting`, `Quit`) to state transitions.

## Blockers
- None. The pure policy logic is fully decoupled from Electron/TAURI side effects.

## Fixture vs Live
- **Fixture**: All tests in `tests/windows_policy.rs` are pure unit tests using the state machine directly. No real browser or window spawning is required.
- **Live**: The eventual integration into `windows.rs` will wire these pure functions to Electron `webContents` events (`will-navigate`, `setWindowOpenHandler`, `render-process-gone`, `unresponsive`). The state machine itself requires no live testing.
