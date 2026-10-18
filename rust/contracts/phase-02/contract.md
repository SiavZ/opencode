# Phase 02 contract: storage, configuration, locations and events

Status: in progress. API crates committed, adapters being built by leaves p02.sqlite, p02.repos, p02.events, p02.config, p02.locations, reviewed by p02.verify.

## Crates and consumers

| Crate | Target | Public contract | Consumers |
| --- | --- | --- | --- |
| `opencode-storage-api` | portable | `Sql`/`SqlExt::savepoint`, `Database`/`DatabaseExt::{transaction, read}`, `TxOptions{behavior, cancel}`, `Value`/`Row`/`FromValue` (+ `get_json`), `StorageError` (Busy/Locked retryable, UniqueViolation/ForeignKey/Constraint conflict, NotFound, Cancelled, ...), `JsonStore`, `records::*` | 04 session-runtime/context, 05, 06 server, event-store, locations |
| `opencode-event-api` | portable | `Definition::{durable, ephemeral}`, `Payload`, `SerializedEvent`, `versioned_type`, `EventStore<Tx>` (publish with in-transaction projectors and commit hook, subscribe/all/all_bounded/durable, read_aggregate, latest_sequence, replay/replay_all, remove, claim), `EventError` | 04, 06 |
| `opencode-storage-sqlite` | native | `SqliteDatabase` implementing `Database` on the baseline Drizzle schema, migrations, repositories, legacy JSON store, credentials | event-store, runtime composition |
| `opencode-event-store` | native | `EventStore<Tx = dyn Sql>` over `Arc<dyn Database>` | runtime composition, 04 via event-api |
| `opencode-config` | native | current ordered documents + legacy discovery, substitution, provenance, secret-safe writes | 03, 04, 05, 06, 07 |
| `opencode-locations` | native | Ref-keyed registry, project identity/migration, workspace lifecycle | 04, 06 |

## Lifecycle invariants

- One transaction context (`&dyn Sql`) is passed to event projectors and commit hooks. The event, sequence row and every projector write commit or roll back together (`packages/core/src/event.ts:205-372`). Wakes and listener notification happen after commit.
- A write is acknowledged only after COMMIT. Cancellation never interrupts after the work closure returned Ok.
- Nested transactions are SAVEPOINTs, matching `packages/effect-drizzle-sqlite/src/effect-sqlite/session.ts:118-187`.
- A replay owner claim is not an execution lease. Neither startup nor replay retries provider work.
- Omitted workspace ID means implicit-local placement.
- Rust reads and writes the same SQLite file and schema as the TypeScript baseline (decision D5).

## Conformance tests

Listed per leaf in `leaves/*.md` and aggregated in `status.md` once leaves complete.
