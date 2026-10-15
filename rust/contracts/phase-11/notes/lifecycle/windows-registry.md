# Windows Registry & State Port Notes

## Inventory Rows

| Baseline File | Lines | Coverage ID | Status |
|---|---|---|---|
| `packages/desktop/src/main/window-registry.ts` | 47 | `cov.p11.window-registry` | Ported |
| `packages/desktop/src/main/window-registry.test.ts` | 91 | `cov.p11.window-registry.test` | Ported |
| `packages/desktop/src/main/window-state.ts` | 29 | `cov.p11.window-state` | Ported |
| `packages/desktop/src/main/windows.ts` (options) | ~100 | `cov.p11.windows.options` | Ported (data models only) |

## Contract

- **Window ID**: Modeled as an opaque `WindowId(String)` to decouple from native toolkit handles.
- **Persistence**: `WindowPersistence` trait abstracts read/write/cleanup. Tests use in-memory `serde_json::Value` mocks.
- **Registry Semantics**:
  - `persisted()`: filters out non-strings and empty strings from stored array.
  - `register()`: adds to internal map, appends to persisted list if not present.
  - `focused()`: tracks last focused window ID.
  - `last_focused()`: returns reference to last focused window, falls back to first available on close.
  - `closed()`: removes from map. If quitting or empty, does NOT cleanup or remove from persisted list (allows relaunch restore). Otherwise, removes from persisted list and triggers cleanup.
- **State Models**: `PersistedWindowState`, `WindowOptions`, `TitleBarStyle`, `TitleBarOverlay`, `ZoomState`, `FullscreenState` provide platform-independent data structures for the shell adapter.
- **Safe URL**: `safe_web_contents_url` and `safe_window_url` return `"<destroyed>"` constant if destroyed, avoiding panics.

## Blockers

| Item | Missing Behavior | Why | Owner | Proof Needed |
|---|---|---|---|---|
| Native display clamping | Bounds clamping to visible work areas | Requires platform-specific screen API (p11.shell) | p11.shell | Integration test with mock screen bounds |
| Actual file persistence | Writing to disk | Requires `dirs` and `std::fs` integration | p11.lifecycle | Live test writing to temp dir |

## Fixture vs Live

- **Fixture**: All current tests (`tests/windows_registry.rs`, `tests/windows_state.rs`) use in-memory mocks (`MockPersistence`, `MockWindow`). This is sufficient for verifying the core logic ported from TypeScript.
- **Live**: Future live tests should use `tempfile::tempdir()` to verify actual JSON file read/write and cleanup behavior for `PersistedWindowState`.
