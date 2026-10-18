# p02.sqlite: native SQLite adapter core

Crate: `rust/crates/storage-sqlite` (connection, transaction, migration, migrations/**). Commits: `baba499a9`, `8504eb1b6`.
Contract implemented: `storage-api/src/{sql.rs,error.rs,value.rs}` (be15cbc5e), unchanged.

## Done

| Item | Where | Proof |
| --- | --- | --- |
| `SqliteDatabase::open/open_in_memory/open_with` implementing `storage_api::Database`. Dedicated worker thread owns a bundled rusqlite `Connection`, bounded queue (256, senders wait), tokio oneshot replies, `close` drains then rejects | `src/connection.rs` | `tests/transactions.rs` (live) |
| Baseline PRAGMAs in TS order (database.ts:26-31) | `connection.rs` `BASELINE_PRAGMAS` | TS-created DB opened by Rust, live interop |
| `PRAGMA journal_mode = WAL` retried within the 5000 ms busy budget. SQLite returns SQLITE_BUSY without calling the busy handler when two connections switch a fresh file at once | `connection.rs` `pragma_with_retry` | `concurrent_open_of_one_path_migrates_once` (was flaky 4/5 before the fix, 15/15 after) |
| Path selection (database.ts:43-55) as pure `database_path(data_dir, channel, &PathEnv)` | `connection.rs` | unit test `path_rules` |
| BEGIN DEFERRED/IMMEDIATE/EXCLUSIVE, `SAVEPOINT effect_sql_N` nesting, Err/panic rollback (`Panicked`), failed COMMIT rolled back | `src/transaction.rs`, `connection.rs` | nested savepoints, panic, deferred-FK failed commit tests |
| Cancellation: token cancelled before BEGIN = `Cancelled` with no work. Mid-statement cancel uses the interrupt handle, then rolls back and returns `Cancelled`. Never interrupts after work returned Ok or during COMMIT | `connection.rs` | 3 cancellation tests |
| Error mapping: BUSY->Busy, LOCKED->Locked, UNIQUE/PK->UniqueViolation, FK->ForeignKey, other constraints->Constraint, READONLY->PermissionDenied, FULL->Full, CORRUPT/NOTADB->Corrupt | `transaction.rs` `map_error` | busy contention (two handles, IMMEDIATE), read-only file tests |
| Read access: `query_only` inside a deferred transaction (single consistent snapshot, writes rejected) | `connection.rs` | `read_access_rejects_writes` |
| `migration::apply/apply_only`, an exact port of migration.ts. Fresh schema plus seeding of all 38 ids, the "not empty and no session" error, named and unnamed `__drizzle_migrations` seeding with strftime prefix matching, one transaction per step together with its journal row, process lock | `src/migration.rs` | `tests/migration.rs` (14), `tests/migration_data.rs` (7). Both port database-migration.test.ts cases |
| All 38 migration bodies, byte-exact SQL extracted from the real TS registry. Both conditional migrations (workspace-name, session-metadata) ported by hand. Like bun `query().all()`, only the first statement per string runs | `src/migrations/registry.json`, `scripts/extract_migrations.ts`, `src/migrations/mod.rs` | registry order test, incremental chain reaches the fresh schema objects |
| WAL-reset fix: bundled SQLite 3.53.2 (rusqlite 0.40.2 / libsqlite3-sys 0.38.2). The fix shipped in 3.51.3 | `lib.rs` `sqlite_version` | `bundled_sqlite_has_wal_reset_fix` |

## Live TS interop (tests/ts_interop.rs, runs real bun from packages/core with temp HOME/XDG)

- A DB created by TS `Database.layerFromPath` opens in Rust: 38 journal ids, no remigration.
- Rows written by Rust are read back by TS with identical JSON (JSON columns, unicode, real, defaults).
- A fresh Rust DB has `sqlite_master` byte-identical to a fresh TS DB, and TS opens it without migrating.
- DBs partially migrated by TS `DatabaseMigration.applyOnly` (1, 10, 24, 33 steps) are migrated forward by Rust and by TS. The resulting `sqlite_master` is identical. Note: the TS harness leaves committed pages in `-wal`, so the test copies the `-wal` file along with the DB.
- If bun or `packages/core/node_modules` is missing, the tests print `SKIP` and pass. On this machine they ran live.

Fixture-only: none. All tests are live against real SQLite files or in-memory DBs.

## Not in this leaf

- Repositories for every baseline table and the legacy JSON store import belong to p02.repos (`src/repositories/**`, `src/legacy_files.rs`). The resume note listed them, but the node spec excludes them.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| none | | | | |

## Not checked

- `packages/opencode/migration` (Drizzle-era journal) is covered only through the shared session-metadata id and the Drizzle journal seeding. No real legacy Drizzle-era DB from an old release was migrated.
- SQLITE_FULL and CORRUPT mappings are checked only by code inspection, not by an induced disk-full or corrupted file.
- Multi-process (not multi-thread) contention against a running TS process.
- Windows paths for `database_path`.
