# p11.lifecycle.deeplinks notes

Owner: p11.lifecycle.deeplinks. Files: `crates/desktop-host/src/deeplinks.rs`, `tests/deeplinks.rs`.

## Contract surface (crate `opencode-desktop-host`, module `deeplinks`)

| Rust item | Baseline | Notes |
| --- | --- | --- |
| `parse_deep_link` | `packages/app/src/pages/layout/deep-links.ts` | Validates scheme `opencode://`, parses `open-project` and `new-session` hostnames, requires non-empty `directory` param. |
| `DeepLinkError` | renderer URL parsing | Typed errors for `InvalidScheme`, `InvalidUrl`, `MissingDirectory`, `UnsupportedHostname`. |
| `DeepLinkSink` | `main/ipc.ts` `sendDeepLinks` | Trait for window routing. Provides `send_deep_links`, `get_focused_window`, `get_most_recent_window`. |
| `DeepLinkManager<S>` | `index.ts` `pendingDeepLinks`, `emitDeepLinks`, `consumeInitialDeepLinks` | Manages pending queue and routing. `emit_deep_links` appends to pending AND attempts delivery. `consume_initial` drains and clears (splice semantics). |
| `DeepLinkManager::extract_from_argv` | `index.ts:205-217` second-instance | Filters `argv` for strings starting with `opencode://`. Preserves baseline ordering and duplicates exactly. |

## Inventory (baseline -> Rust -> test, all fixture/in-process)

| Baseline | Rust | Test |
| --- | --- | --- |
| `index.ts:70-87` `emitDeepLinks` | `DeepLinkManager::emit_deep_links` | `tests/deeplinks.rs::emit::appends_to_pending_and_delivers_to_focused` |
| `index.ts:205-217` second-instance argv | `DeepLinkManager::extract_from_argv` | `tests/deeplinks.rs::argv::extracts_opencode_urls_from_argv` |
| `index.ts:218-223` `open-url` | (handled by shell, feeds `emit_deep_links`) | N/A (shell responsibility) |
| `index.ts:295` `consumeInitialDeepLinks` | `DeepLinkManager::consume_initial` | `tests/deeplinks.rs::emit::consume_initial_is_splice_semantics` |
| `main/ipc.ts` `sendDeepLinks` | `DeepLinkSink::send_deep_links` | `tests/deeplinks.rs::emit::preserves_baseline_duplicate_and_ordering` |
| `packages/app/src/pages/layout/deep-links.ts` parsing | `parse_deep_link` | `tests/deeplinks.rs::parse::*` (10 tests) |

## Validation gaps (baseline does NOT do)

1. The Electron main process does NOT validate URLs beyond `arg.startsWith("opencode://")`. It blindly passes them to the renderer.
2. The renderer does the actual URL parsing and validation (`URL.canParse`, hostname check, directory presence).
3. The Rust implementation provides `parse_deep_link` for the shell/renderer IPC boundary to optionally validate, but the raw `emit_deep_links` accepts any `String` to preserve the baseline "pass-through" behavior.
4. Baseline does NOT deduplicate pending links. The Rust implementation preserves this: duplicates are appended and delivered as-is.

## Fixture vs live

All tests are in-process fixture tests using `FixtureSink`. No real window registry or IPC is touched. The shell binary will provide the real `DeepLinkSink` implementation.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Shell integration | wiring `DeepLinkManager` to actual window registry and IPC | needs desktop shell bin to implement `DeepLinkSink` | p11 shell / bin owner | end-to-end desktop launch test with `opencode://` URL |
| macOS `open-url` event | native event handling | platform-specific, belongs in shell bin | p11 shell / platform owner | macOS `open-url` test in shell |
