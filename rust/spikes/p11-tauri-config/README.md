# p11-tauri-config Spike

This spike documents the Tauri 2 configuration and wiring decisions for the OpenCode desktop shell.

## Configuration
- `tauri.conf.json` defines the bundle targets: `dmg`, `zip` (macOS), `nsis` (Windows), `appimage`, `deb`, `rpm` (Linux).
- Deep link scheme: `opencode://`
- Single instance: enforced via `tauri-plugin-single-instance`.

## Security
- `withGlobalTauri` is `false`.
- CSP is explicitly managed (null in placeholder, to be tightened in production).
- No real signing or publishing is configured; placeholders are used for icon paths.

## Bridge
- The `opencode-desktop-host` crate provides the capability bridge.
- IPC channels are dispatched via `opencode_desktop_host::bridge::dispatch`.
- Lifecycle readiness is managed by `opencode_desktop_host::lifecycle::readiness`.
- The shell binary is the only component that depends on `tauri`, keeping wasm-compiled crates clean.

## Smoke Test
- Running `opencode-desktop --smoke-test` creates a window, logs `window-created`, waits 500ms, and exits gracefully with code 0.
- This provides a fast runtime validation path without requiring a full UI build.
