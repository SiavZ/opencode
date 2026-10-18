# p02.events leaf: durable event store (`opencode-event-store`)

Status: implemented and tested (2026-10-03). Owner: phase-02 (leaf p02.events).

## What exists

- `SqlEventStore::new(Arc<dyn Database>)` / `with_options(db, Options { before_aggregate_read })` implements `opencode_event_api::EventStore` (contract v2, 849d43b09: no `Tx` associated type, `Projector`/`CommitHook` take `&dyn Sql` with higher-ranked lifetimes).
- `register(&[Definition])` loads the durable manifest (stored `type.version` -> definition). `project()` and `publish()` register their definition implicitly. Replay or durable reads of an unregistered stored type fail with `Unknown durable event type <type>`, like `Durable.get` in the baseline.
- Modules: `commit.rs` (`commitDurableEvent`, packages/core/src/event.ts:205-372), `hub.rs` (listeners, typed/all/bounded subscribers, per-aggregate wakes), `read.rs` (`readAfter`, `durable()`, `readAggregate`, `latestSequence`), `store.rs` (service, publish/replay/replayAll/remove/claim).

## Baseline parity

| Behavior | Baseline | Rust |
| --- | --- | --- |
| One IMMEDIATE transaction: owner checks, exact duplicate no-op with owner backfill, divergence, non-strict owner skip, sequence gap, duplicate ID, projectors, commit hook, `event_sequence` upsert (owner only when previously null), `event` insert | event.ts:226-353 | commit.rs:133 |
| Aggregate field and envelope mismatch before the transaction | event.ts:211-232 | commit.rs `aggregate_of` |
| Wakes only after commit, listener isolation for durable, propagation for live-only | event.ts:354-418 | store.rs:231, hub.rs:163 |
| Commit hook on live-only event fails `Local commit hooks require a durable event` | event.ts:378-385 | store.rs:231 |
| replay / replayAll same-aggregate and contiguous checks, per-event commit | event.ts:446-520 | store.rs:349 |
| remove (one transaction), claim (UPDATE only) | event.ts:522-540 | store.rs:386, 405 |
| `durable()` catch-up then sliding(1) wake | event.ts:580-617 | read.rs:209, hub.rs:152 |
| `readAggregate` limit+1 `has_more`, type filter | event.ts:70-109 | read.rs:123 |
| `allBounded` dropping queue, overflow ends with `SubscriberOverflow` | event.ts:146-160 | hub.rs:104 |

Error messages are byte-identical (asserted in tests).

## Tests (46, all real SQLite via storage-sqlite, no fixtures)

- `tests/commit.rs` (13): projectors inline, commit hook ordering, rollback of projector + hook + rows on hook or projector failure, listener order, durable listener isolation (error and panic), live-only fail-fast, notify-after-commit, seq per aggregate, custom aggregate field, versioned type.
- `tests/replay.rs` (10): replay through projectors, envelope aggregate mismatch, sequence gap, unknown type, replayAll validation and chunks, replayAll partial failure keeps earlier events committed, exact duplicate suppressed, divergence (data and id) not published, ID reuse, remove then fresh replay.
- `tests/owners.rs` (7): claim fences non-strict replay, strict owner on exact replay (incl. `got none`), exact replay backfills owner, replay claims new and existing unowned sequences, strict conflict, other owner leaves sequence unchanged, absent-row claim no-op.
- `tests/streams.rs` (11): typed/wildcard, projectors before streams, bounded overflow isolation, durable tail after seq, handoff race, paused historical read retains wake (`before_aggregate_read`), 64-event wake coalescing, live-only omission, 4 concurrent writers vs one durable reader see every seq once, readAggregate paging, dropped streams.
- `tests/rows.rs` (3): LIVE: TS `EventV2.publish` via bun and Rust write byte-identical `event`/`event_sequence` rows (unicode, escapes, nested key order, 1e21), and Rust `durable()` reads TS-written rows; compact JSON key order; pinned divergence below.
- `src/hub.rs` unit (2): wake unregister on drop, sliding(1) permit, unsubscribe after drop.

Run: `cargo test -p opencode-event-store` (live row test skips with a message without bun/node_modules; `OPENCODE_REPO` overrides the repo root).

## Blockers and known divergences

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Rust-built integral floats | `Value::json` prints `-0.0`/`100.0` where JS prints `0`/`100` | serde_json float formatting; data decoded from TS JSON is unaffected | storage-api (p02) | flip `rust_built_integral_floats_diverge_from_js_number_formatting` |
| Durable manifest | schema crate `event_manifest.rs` is empty, so composition must call `register` with every durable definition | p01 schema manifest in progress | p01 / runtime composer | runtime registers `Durable` set; replay of every baseline type |
| Location default | baseline fills `location` from the ambient `Location.Service`; Rust takes it only from `PublishOptions.location` | no ambient service in Rust; callers pass it | runtime (p91) | server publish paths pass location |
| Codec | encode/decode only via `Definition::with_codec`; no Effect Schema transforms (e.g. DateTime) | schema codecs per definition are p01/p04 work | definition owners | replay of `session.next.context.updated` decodes timestamp |
