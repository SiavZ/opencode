# p11.persistence notes (drafts, stores, attachments, migration, install state, debug logs)

Owner: p11.persistence. Adopted commits: `ce974c9c7` (modules + tests), `7c3054025` (initial notes),
`0139b193f` (zip backend fix by previous owner after notifying p11 root). Resumed followthrough:
`4637e7f5c` (joined draft timers, attachment race hardening, updater/WAL migration).

## Contract surface (crate `opencode-desktop-host`)

| Rust item | Capability / IPC channel | Notes |
| --- | --- | --- |
| `drafts::DraftStore::{open, open_in, get, set, delete, put_blob, get_blob, flush, close, flush_and_join, close_and_join}` | `draft-get`, `draft-set`, `draft-delete`, `draft-blob-put`, `draft-blob-get` | Shell must stop admission, call `flush_and_join()` on async before-quit/session-end, then `close_and_join()` on final shutdown. Synchronous flush/close and Drop retain compatibility fallback behavior. |
| `store::StoreRegistry::{ipc_get, ipc_set, ipc_delete, ipc_clear, keys, len}` | `store-get/set/delete/clear/keys/length` | `ipc_get` returns `None` for any error, matching the baseline try/catch. |
| `store::StoreRegistry::{get, set, has, delete, clear, remove_file, remove_file_if_empty}` | host-internal settings (`getStore()`) | Typed `serde_json::Value`, used for settings keys in `store::keys`. |
| `store::cleanup::{cleanup_store_files, delete_store_file_if_empty}` | startup cleanup (`index.ts:258`) | |
| `store::install_state::{has_existing_app_state, initialize_old_layout_eligibility, is_old_layout_eligible}` | onboarding (`onboarding.ts:12-25`) | Must run before the host writes anything into userData. |
| `store::debug_logs::{LogDirs, export_debug_logs, tail, server_log_roots, stamp, safe_log_name}` | `export-debug-logs` | The shell provides the Downloads dir, process facts, net-log stop/restart and the reveal in the file manager. |
| `attachments::{AttachmentPicker, PickedFiles, FileDialog, read_attachment, assert_attachment_budget}` | `open-file-picker`, `read-picked-file`, `release-picked-files`, `open-directory-picker`, `save-file-picker` | `FileDialog` is implemented by the shell (native dialog). `SenderId = u64` is the webview id from the bridge. `release_sender` should be called when a webview is destroyed. |
| `migrate::{migrate_tauri, migrate_electron, tauri_dir, electron_user_data_dir, Channel, DirEnv, Os}` | startup (`index.ts:257`) | Sources are parameters, so tests never resolve real directories. |

Wire shapes: `PickedSelection {token, files:[{path,name,size}]}`, `DirectorySelection` is a string or a string[] (untagged),
options use camelCase (`defaultPath`). Store files are tab-indented `JSON.stringify` output, the name is the file name
(`fileExtension: ""`), and property order follows JS rules (integer keys first).

## Inventory (baseline -> Rust -> test)

| Baseline | Rust item | Test (fixture unless noted) |
| --- | --- | --- |
| `desktop/src/main/draft-store.ts:16-21` schema, WAL | `DraftStore::open`, `SCHEMA` | `tests/drafts.rs::schema_and_wal_match_baseline_and_close_flushes` |
| `draft-store.ts:22-35` blob GC at open | `drafts::collect_garbage` | `drafts.rs::reopen_collects_unreferenced_blobs_only`, `corrupt_document_fails_open_without_deleting_blobs` |
| `draft-store.ts:36-56` pending map, 500 ms timer, transactional flush | `DraftStore::{set, flush, schedule}`, `FLUSH_DELAY` | `drafts.rs::timer_flushes_after_delay_once` (tokio paused clock), `pending_delete_hides_stored_value_until_flushed` |
| `draft-store.ts:57-61` get reads pending first | `DraftStore::get` | `drafts.rs::flushes_the_latest_buffered_draft_and_stores_blobs` (port of `draft-store.test.ts:4-16`) |
| `draft-store.ts:62-71` sha256 blob ids, insert-or-ignore | `DraftStore::{put_blob, get_blob}` | `drafts.rs::blob_ids_are_sha256_hex_and_idempotent` |
| `draft-store.ts:72-80` close flushes | `DraftStore::{close, close_and_join, flush_and_join}`, owned timer JoinSet | `drafts.rs::drop_flushes_pending_writes`, `joined_shutdown_flushes_before_the_timer_and_closes_all_clones`, `timer_rearms_after_previous_flush_and_joined_flush_keeps_store_open` |
| `ipc.ts:58-63,143-150` draft IPC + quit flush | `DraftStore` API, shell wiring | `drafts.rs` (the shell wiring is not tested here) |
| `store.ts:15-27`, conf 15.1.0 get/set/delete/clear/store | `StoreRegistry` | `tests/store.rs::writes_conf_compatible_tab_indented_file_without_extension`, `reads_existing_electron_file_and_preserves_internal_key`, `keys_follow_js_property_order_and_proto_keys_are_ignored`, `concurrent_writers_do_not_lose_keys` |
| `store.ts:28-35` remove file (if empty) | `StoreRegistry::{remove_file, remove_file_if_empty}` | `store.rs::delete_and_clear_remove_empty_scoped_store_files_only` |
| `ipc.ts:114-142` store IPC | `StoreRegistry::ipc_*` | `store.rs::ipc_get_stringifies_non_strings_and_maps_errors_to_null` |
| `store-keys.ts:1-7` | `store::keys` | used by all store tests |
| `store-cleanup.ts:17-61` | `cleanup::cleanup_store_files` | `tests/store_cleanup.rs` (all 4 baseline tests ported, plus `emptiness_rules_match_baseline`, `missing_directory_scans_nothing`) |
| `store-cleanup.ts:63-94` | `cleanup::{delete_store_file_if_empty, store_kind, is_empty_store}` | `store_cleanup.rs::removes_a_scoped_store_immediately_when_it_becomes_empty` |
| `install-state.ts:1-8` | `install_state::has_existing_app_state` | `tests/store_install_state.rs` (both baseline tests) |
| `onboarding.ts:12-25` eligibility | `install_state::{initialize_old_layout_eligibility, is_old_layout_eligible}` | `store_install_state.rs::eligibility_is_computed_once_and_persisted` |
| `attachment-picker.ts:7-32` token authorizations | `PickedFiles` | `tests/attachments.rs` (4 baseline authorization tests) |
| `attachment-picker.ts:34-38` budget | `assert_attachment_budget` | `attachments.rs::accepts_*`, `rejects_the_selection_*` |
| `attachment-picker.ts:40-57` bounded read | `read_attachment` | `attachments.rs::reads_an_approved_file_*`, `rejects_an_oversized_file_*` |
| `ipc.ts:28-31,151-209` pickers | `AttachmentPicker::{open_file_picker, open_directory_picker, save_file_picker}`, `FileDialog` | `attachments.rs::open_file_picker_*`, `directory_and_save_pickers` (scripted dialog fixture) |
| `desktop-native.ts:292-321` picker strings | `PickerMessages` | `attachments.rs::localized_messages_are_used` |
| `migrate.ts:13-34` tauri dir / app id | `migrate::{tauri_dir, Channel::app_id}` | `tests/migrate.rs::resolves_tauri_and_electron_dirs_per_os` |
| `migrate.ts:56-64`, Object.entries on non-object roots | `merge_file` handles arrays, BMP strings, numbers and booleans, null is an error before the completion flag | `migrate.rs::tauri_non_object_entries_match_javascript_and_preserve_index_order`, `null_tauri_source_fails_without_marking_migration_complete` |
| `migrate.ts:39-91` | `migrate::migrate_tauri` | `migrate.rs::tauri_migration_merges_without_overwriting_and_runs_once`, `tauri_migration_without_source_sets_flag` |
| new: Electron userData import (`index.ts:125,143-146` layout) | `migrate::{migrate_electron, electron_user_data_dir}` | `migrate.rs::electron_import_copies_stores_window_state_and_drafts`, `electron_import_in_place_is_a_noop` |
| `attachment-picker.ts:19-25` concurrent read race (security discrepancy) | `PickedFiles::read`, `attachments::authorization::ReadLease` | `tests/attachments_concurrency.rs` (aggregate budget, sender revocation, cancellation, valid concurrent reads) |
| `updater.ts:28`, Electron extensionless updater store | `migrate_electron` imports `opencode.updater` without overwriting | `migrate.rs::electron_import_preserves_updater_store_and_source_wal_without_overwriting` |
| Electron source WAL documents/blob consistency | `migrate_drafts` read-only source transaction | `migrate.rs::electron_import_preserves_updater_store_and_source_wal_without_overwriting` |
| `logging.ts:55-75,107-180` export, stamp, collect, manifest | `debug_logs::{export_debug_logs, stamp, LogDirs}` | `tests/store_debug_logs.rs::export_bundles_recent_files_with_manifest`, `stamp_and_safe_names_match_baseline` |
| `logging.ts:93-99,118-131,152-155` | `debug_logs::{tail, LogDirs::cleanup, server_log_roots}` | `store_debug_logs.rs::tail_*`, `cleanup_*`, `server_log_roots_*` |

All tests are fixture-based (temp dirs, `:memory:` SQLite, scripted dialog). None are live. Nothing reads real
`~/Library`, `%APPDATA%` or XDG data.

## Deliberate differences (need review)

| Item | Baseline | Rust | Why |
| --- | --- | --- | --- |
| Store name validation | any name joined onto userData (path traversal possible from renderer) | `InvalidName` for `/`, `\`, `..`, empty, absolute | sender/path hardening required by phase plan |
| Store write | `atomically` temp + rename, in-process cache | temp + fsync + rename, file re-read on every op, per-name lock | there is no stale cache across processes |
| Attachment tokens on renderer destroy | leaked | `PickedFiles::release_sender` | bounded cleanup |
| Electron import | none (Electron *is* the host) | `migrate_electron` with `electronMigrated` flag | the native host may use a different userData. If the shell reuses the Electron dir, the import is a no-op |
| Attachment concurrent completion | overlapping reads can each use an old remaining budget, and released selections still return in-flight bytes | recheck actual bytes under the selection lock before delivery, revoked token fails, RAII releases cancelled reads | explicit sender/budget hardening, two failing-before regressions captured on 2026-10-03, Phase 90 review pending |
| Draft task lifecycle | JS one-thread timer has no join boundary | owned JoinSet plus async flush/close joining, DB/pending lock ordering prevents transient stale reads | Rust concurrent lifecycle requirements |
| Debug export | electron-log scopes, netLog, crashpad | filesystem bundle only | Electron-only facilities, see blockers |

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Quit flush wiring | `flush_and_join` / `close_and_join` after stopping prompt/draft admission | lifecycle/shell not yet composed in this node | p11.lifecycle / p11.shell | shell integration test that kills after set and reopens |
| Native dialog | `FileDialog` impl (macOS/Windows/Linux) | lives in `bins/desktop` | p11.shell | real dialog on each OS |
| Sender identity | `SenderId` mapped to the bridge's webview id | bridge type not yet published | p11.lifecycle (bridge) | bridge dispatch test using `AttachmentPicker` |
| Log writer | tracing subscriber writing `LogDirs::file_for(scope)` with 5 MiB rotation, console EPIPE handling | electron-log replacement belongs to the shell | p11.shell | packaged run producing logs, then an export |
| Net log / crashpad | Chromium net-log capture and crash dumps in the export | Electron-specific, with no webview equivalent decided | p11.shell / Phase 90 | decision record |
| userData choice | whether the native host reuses the Electron dir (in-place) or imports | product decision | p11 root | decision plus `migrate_electron` call site |
| Draft crash recovery | real kill -9 mid-flush on packaged app | needs the packaged shell | p11.verify | crash-recovery run |
| Tauri top-level string with surrogate pairs | typed `MigrateError::Unsupported`, does not mark migration complete | JS Object.entries emits lone UTF-16 surrogate strings that serde_json::Value cannot represent | p11 persistence / Phase 90 serialization | approved UTF-16 JSON strategy, then surrogate-pair roundtrip fixture |
| Forced focus debugger | `debug.ts:16-95` CSS.forcePseudoState and debugger ownership | Chromium debugger API has no implemented WebKit adapter | p11.shell / Phase 90 | decision and supported-webview test |
| Strict crate lint | unrelated connections/platform lint failures | cannot edit sibling ownership | p11.connections | fix health.rs:56, apps.rs:104,111, wsl/exe.rs:145 then rerun -D warnings |

## Resumed validation (2026-10-03)

```sh
cd rust
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export SCCACHE_BASEDIRS="$PWD/target" CARGO_TARGET_DIR="$PWD/target/p11"
cargo test -p opencode-desktop-host --test drafts --test store --test store_cleanup \
  --test store_install_state --test store_debug_logs --test attachments \
  --test attachments_concurrency --test migrate
cargo check -p opencode-desktop-host
cargo clippy -p opencode-desktop-host -- -D warnings
```

All inherited 47 tests passed before changes. Followthrough brings the fixture acceptance total to **57**: attachments 12, concurrency 4, drafts 10, migrate 9, store 8, cleanup 6, debug logs 5, install state 3. The final aggregate run at 18:48 UTC passed all 57 cases on the completed tree, followed by host check and scoped lint with the documented sibling-only allowances. Host check passes. Two newly written concurrency regressions first failed against the adopted implementation, proving aggregate-budget and sender-revocation gaps, then passed after correction. The four concurrency cases also cover cancellation cleanup and successful same-token overlap within budget.

Strict clippy remains blocked by four unrelated sibling locations listed above. Linting all eight persistence integration targets with only `-A clippy::collapsible_if -A clippy::chunks_exact_to_as_chunks` after `-D warnings` passes. These allowances are command-line validation workarounds, not source-level suppressions. Owned Rust files were rustfmt formatted and stay under 500 lines. No manifests or lib.rs were changed during resumed work.

Remaining acceptance not checked: packaged quit/session-end routing, native dialog interaction, bridge-to-picker sender identity, real user directory migration, kill -9 / power-loss recovery, cross-process or cross-StoreRegistry concurrent writes, malformed UTF-16 JSON, exhaustive JavaScript floating-point serialization, filesystem symlink replacement races and actual log rotation/Chromium facilities. Fixture source WAL migration runs with the source database open and preserves logical source content, but does not prove concurrent mutation while rows are imported. `set` admission must stop before joined close. Synchronous close/Drop abort timers and flush but cannot await task joins, so production async shutdown must use the joined API.
