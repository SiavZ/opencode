# p02.repos: repositories, credentials, legacy JSON store, legacy auth

Commits: `3a4a25460` (records), `b2eb92971` (repositories + credentials), `e201b4040` (legacy files), `5122c1e4d` (tests), `8a31006e0` (TS interop).

## Done

| Item | Where | Proof |
| --- | --- | --- |
| Typed row records for all 19 baseline tables (Drizzle column order, PK). The `TableRecord` trait. No business logic for session/message/part/todo/session_message/session_input/session_context_epoch/event/event_sequence | `storage-api/src/records/mod.rs` | `records_follow_drizzle_column_order` |
| Drizzle codecs: JSON mode (compact `JSON.stringify`), boolean mode (writes 0/1, reads `Number(v) === 1`), `absolute`/`directory`/`path`/`absoluteArray` path columns from database/path.ts, plus posix and windows rules | `records/codec.rs`, `records/path.rs` | `project_round_trips_with_drizzle_encodings`, `relative_paths_are_rejected_before_writing`, `session_row_path_columns`, `control_account_boolean_mode`, unit tests in `path.rs` |
| Generic `Crud<T>` over `&dyn Sql`. Like drizzle `buildUpdateSet`, every UPDATE and onConflictDoUpdate sets `time_updated = Date.now()` unless the caller sets it | `records/crud.rs` | `project_update_sandboxes_and_initialized`, `session_share_upsert` |
| `Secret<T>` with redacted `Debug`, used for tokens, share secrets and credential values | `records/secret.rs` | `account_persist_activate_and_remove`, `credential_debug_redacts_secrets` |
| `localeCompare` collation (ICU root, ASCII order captured from bun) | `records/collation.rs` | unit test, `workspace_list_uses_locale_order`, `list_uses_locale_compare` |
| project: get/list/insert/upsert (fromDirectory, `undefined` optionals left alone)/update/setInitialized/add+remove sandbox/adopt global sessions/migrateID | `storage-sqlite/src/repositories/projects.rs` | `tests/repositories.rs` |
| project_directory: create ignore/replace with setWhere, remove, list (time_created DESC, directory ASC), get, contains | same | `project_directory_create_replace_and_order` |
| permission (`psv_` ids, onConflictDoNothing), workspace (list sorted by localeCompare), account (+state id 1, persist with `normalizeServerUrl`, persistToken, remove clears state), control_account (legacy read), session_share upsert, data_migration | `repositories/domains.rs` | `tests/repositories.rs`, `tests/repositories_accounts.rs` |
| Credentials: port of credential.ts:51-138. Rows without integration_id stay hidden, lists ordered by time_created ASC, create rotates (delete+insert in a savepoint), label defaults to "default", the `!label && !value` no-op, an undecodable value raises `Decode` | `repositories/credentials.rs` | `tests/credentials.rs` |
| Legacy auth.json: port of auth/index.ts:52-95. `OPENCODE_AUTH_CONTENT` override (explicit input, never read implicitly in tests), schema filtering, trailing-slash normalization, 2-space JSON, chmod 0600, the "Failed to write auth data" error, redacted Debug | `legacy_files/auth.rs` | all 4 auth.test.ts cases plus format/perms/env/failure tests in `tests/credentials.rs` |
| Legacy JSON store: implements `JsonStore`. `path.join` layout, per-file RwLock (dropped when idle like RcMap ttl 0), `NotFound(<file>)` with message `Resource not found: <file>`, missing remove succeeds, list of a missing prefix is empty, list sorted with localeCompare, 2-space JS-number JSON | `legacy_files/store.rs`, `legacy_files.rs` | `tests/legacy_json.rs`, ports of storage.test.ts:63-191 |
| Store migrations 1 and 2 with their own `storage/migration` text journal (parseInt semantics, a failure stops the loop without advancing). Migration 1 uses a real `git rev-list --max-parents=0 --all` subprocess with the TS `-c` flags | `legacy_files/migrate.rs` | `migration_2_with_invalid_marker`, `migration_2_skips_invalid_summaries`, `migration_1_tolerates_malformed_records` (real git repo), `failed_migration_keeps_marker` |

## Live vs fixture

- All tests are live: a real migrated SQLite (`SqliteDatabase::open_in_memory`/`open`), real temp dirs and a real `git`.
- Live TS interop (`tests/repositories_interop.rs` + `tests/ts/repos.ts`, bun from packages/core with temp HOME/XDG/OPENCODE_DB):
  - Rust writes credential, permission and directory rows, and TS `Credential.all`, `PermissionSaved.list` and `ProjectDirectories.list` read them back exactly.
  - TS writes the same kinds of rows and the Rust repositories read them.
- Skipped with a `SKIP` message when bun or git is missing. Both ran live here.

## Known divergences (documented in headers)

- When `OPENCODE_AUTH_CONTENT` holds a non-object JSON value, Rust returns `{}`. TS would hand the raw value to callers, which then break.
- A negative store migration marker is clamped to 0. TS would index past the array and crash.
- `localeCompare` for non-ASCII sorts by code point after all ASCII. This is an approximation; keys and ids are ASCII in practice.
- Floats outside [1e-7, 1e21) in legacy JSON print in ryu form, not JS exponent form.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| none | | | | |

## Not checked

- Windows path codecs were only checked through the pure `*_for(windows=true)` unit tests, not on a real windows build.
- Byte-for-byte equality of a JSON store file written by TS vs Rust for the same value. The formatting rules are covered, but no cross-run diff was taken.
- Session-runtime semantics (Phase 04) for session/message/part rows. Only plain records are provided.
- Concurrency of the legacy store across processes (TS locks are also process-local).
