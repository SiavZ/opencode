# Menu System Port (Phase 11)

## Inventory Rows
- `crates/desktop-host/src/menu.rs`: Complete port of `packages/desktop/src/main/menu.ts`, `desktop-menu-actions.ts`, `native-translations.ts`, and `packages/app/src/desktop-menu.ts` + `i18n/desktop-native.ts`.

## Contract
- **Data Model**: Toolkit-independent `MenuCatalog` with `Menu`, `MenuEntry` (Item/Separator), `MenuAction`, `MenuRole`, `Accelerators`, and `EnabledCondition`.
- **Platform Layouts**: `Platform` enum (MacOS, Windows, Linux). `is_visible_on` filters menus and items per platform (e.g., App menu is macOS-only).
- **Translations**: `TranslationManager` with `TranslationBundle` provides `translate(key)` with automatic English fallback for missing keys, matching `formatDesktopNativeMessage` behavior for `{{param}}` substitution.
- **Action Routing**: `MenuCommandSink` trait for sending commands to focused window (matching `sendMenuCommand` in `main/ipc.ts`), and `MenuActionHandlers` trait for host-level side effects (`check_for_updates`, `relaunch`). `dispatch_menu_click` routes entries to the correct handler.

## Blockers
- None. The module is fully implemented and tested. Toolkit adapters (e.g., Tauri, Wry) will consume this catalog and implement the `MenuCommandSink` and `MenuActionHandlers` traits.

## Fixture vs Live
- **Fixture**: Unit tests in `menu.rs` verify catalog shape (7 menus), platform visibility, action/command roundtrip, and translation fallback.
- **Live**: Integration with actual windowing toolkit is deferred to the toolkit adapter layer (owner: phase-11 toolkit nodes), which will instantiate the catalog and wire the sink/handlers to real IPC or native APIs.

## Exact Keys Recorded
All 66 translation keys from `DESKTOP_NATIVE_ENGLISH` are embedded in `english_messages()`, including:
- Menu labels: `desktop.menu.app`, `desktop.menu.file`, `desktop.menu.edit`, `desktop.menu.view`, `desktop.menu.go`, `desktop.menu.window`, `desktop.menu.help`, etc.
- Updater dialogs: `desktop.updater.dialog.*`
- Recovery: `desktop.recovery.*`
- Dialogs: `desktop.dialog.*`, `desktop.server.local`
- WSL errors: `desktop.wsl.error.*`
- Picker errors: `desktop.picker.error.*`
