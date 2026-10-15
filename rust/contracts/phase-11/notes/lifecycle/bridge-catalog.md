# Bridge Catalog Notes

## Inventory Rows

| Baseline File:Line | Rust Item | Test |
|---|---|---|
| `packages/desktop/src/preload/types.ts:45-116` | `IpcChannel` enum, payload structs | `tests/bridge_catalog.rs::test_all_channels_cover_baseline` |
| `packages/desktop/src/preload/index.ts:13-136` | `IpcChannel::name()`, `from_name()`, `all()` | `tests/bridge_catalog.rs::test_channel_name_round_trip` |
| `packages/desktop/src/main/ipc.ts:57-300` | `ChannelKind` (Invoke/Send/Event) | `tests/bridge_catalog.rs::test_channel_kinds` |
| `packages/desktop/src/main/wsl/ipc.ts:8-104` | WSL-specific `IpcChannel` variants | `tests/bridge_catalog.rs::test_all_channels_cover_baseline` |
| N/A (New Rust API) | `Dispatcher` trait, `DispatchError`, `SenderContext` | `tests/bridge_dispatch.rs` (all tests) |

## Contract Surface

The bridge catalog provides a single source of truth for all IPC channel names, directions, and payload shapes between the renderer (preload) and main process.

- **`IpcChannel`**: Enum with 75 variants covering all preload invoke/send channels and main->renderer event channels, plus an `Unsupported` variant for discrepancies.
- **`ChannelKind`**: Enum distinguishing `Invoke` (request-response), `Send` (fire-and-forget), and `Event` (main-to-renderer).
- **`Dispatcher` trait**: Defines the capability dispatch boundary. Implementors (e.g., Tauri adapter or shell) must provide `is_authorized` and can override `max_payload_size`.
- **`DispatchError`**: Typed errors (`NotFound`, `Unauthorized`, `PayloadTooLarge`, `InvalidArgs`, `Unsupported`, `Failed`) for precise error handling.
- **Payload Structs**: Common types like `ServerReadyData`, `TitlebarTheme`, `FatalRendererError`, `OpenFilePickerOpts`, `PickedFile`, `OpenFilePickerResult`, `SaveFilePickerOpts`, `OpenPathOpts`, `ClipboardImage`, `DesktopMenuActionPayload`.

## Blockers Table

| Item | Missing Behavior | Why | Owner | Proof Needed |
|---|---|---|---|---|
| `install-cli` | Handler implementation in Rust | Ported to catalog but actual sidecar CLI install logic not yet in `desktop-host` | p11.lifecycle | Integration test showing CLI binary placement |
| Binary payloads (`draft-blob-*`, `read-clipboard-image`) | Efficient binary serialization | `serde_json` serializes `Vec<u8>` as base64 array; may need `serde_bytes` later for performance | p11.lifecycle | Benchmark or explicit decision in modularity review |
| `DesktopMenuAction` payload | Exact enum mapping | Currently flattened as `action: String` + `extra: Value`; needs full enum port from `@opencode-ai/app/desktop-menu` | p11.lifecycle | Type-check passing against app desktop-menu types |

## Fixture vs Live Labels

- **Fixture**: `tests/bridge_catalog.rs` and `tests/bridge_dispatch.rs` use mock dispatchers and hardcoded baseline channel lists. No real IPC or filesystem side effects occur.
- **Live**: None in this node. Live IPC integration tests will be added when the shell/Tauri adapter implements the `Dispatcher` trait and wires it to actual `ipcMain` handlers.
