# p04.admission child note

Owner node: p04.admission. Files: `crates/session-runtime/src/{admission,inbox,projection,store,service}.rs`, `src/projection/{adapter,legacy,updater}.rs`, `src/service/util.rs`, `tests/admission*.rs`, `tests/support/sql.rs`.

## What is implemented

| Area | Rust | Baseline |
| --- | --- | --- |
| Prompt admission | `admission::admit`, `admission::resolve_prompt` | `session/input.ts:41-81` `admit`, `session.ts` `resolvePrompt` |
| Historical message-only retry repair | `admission::lookup` (one immediate tx) | not in baseline, required by AGENTS.md (see below) |
| Inbox seams for the runner | `inbox::{has_pending, promote_steers, promote_next_queued, find, from_row}` | `session/input.ts:170-288` |
| Inbox projections | `inbox::{project_admitted, project_prompted}` | `session/input.ts:83-168` |
| Session projectors | `projection::register_projectors`, `LocalSessionEvents::install` | `session/projector.ts` (all `events.project` calls, including legacy `session.created/updated/deleted`, moved, revert staged/cleared/committed) |
| Transcript folding | `projection/updater.rs` + `projection/adapter.rs` (legacy session rows in `projection/legacy.rs`) | `session/message-updater.ts` `update` + projector SQL adapter |
| Publish helpers for runner/lifecycle/legacy | `projection::{publish, publish_session_event, definition_of, event_error, storage_error}`, `SessionEvents` trait | `events.publish` |
| Reads | `store::SessionStore::{get, message, context, list, messages}`, `store::info_from_row` | `session/store.ts`, `session/info.ts`, `session.ts` list/messages |
| Service | `service::LocalSessionService` implements `session_api::operations::SessionService`; `events()` stream; `SessionLocations` port for project resolution and Location revert | `session.ts` layer |

Seam signatures match contract.md exactly. `SessionEvents` is a trait (`database()`, `store()`); `LocalSessionEvents` is the concrete facade.

## Historical retry discrepancy (D4) resolution

Reproduced: `tests/admission_historical.rs::reproduces_baseline_admit_failure_on_message_only_history` builds the message-only state (visible `user` row projected by `session.next.prompted`, inbox row absent like a pre-inbox database) and shows the baseline path (publish `prompt.admitted`) fails with `ProjectionError::LifecycleConflict` and rolls back.

Resolved: `admit` runs `lookup` in one `BEGIN IMMEDIATE` transaction: inbox hit returns the row; else a visible message with the same ID is inspected. Same Session, type `user`, and identical encoded prompt (`text/files/agents`) inserts a promoted inbox row (`admitted_seq = promoted_seq = message.seq`, `time_created = message.time_created`, delivery = requested delivery). Anything else is `PromptConflict`. No durable event is written by the repair. Tests: exact steer/queue retries, idempotent second retry, attachments in equivalence, conflicts (other prompt, other Session, non-user message) write nothing, and 10 concurrent retries across two independent DB handles reconcile exactly once.

## Tests

All under `crates/session-runtime/tests/`, real migrated SQLite (`opencode-storage-sqlite` `SqliteDatabase::open`, baseline migrations) and the real p02 event store (`opencode_event_store::SqlEventStore`, event-api contract v2, dev-dependency). 33 tests pass (15 admission, 6 historical, 4 inbox, 5 projection, 3 unit), stable over 5 repeated runs.

- `admission.rs`: ports of `packages/core/test/session-prompt.test.ts` prompt cases plus create adoption (sequential and concurrent across two handles), switch model/agent, unavailable ops, resume/interrupt delegation.
- `admission_historical.rs`: D4 proof above.
- `admission_inbox.rs`: steer cutoff, queue FIFO, concurrent promotion once, revert-commit pruning.
- `admission_projection.rs`: assistant turn folding, stale assistant superseded, tool failure states, shell, cursors, list anchors/search, history paging, decode errors.
- Unit tests in `admission.rs` (MIME resolution) and `service.rs` (relative path, ISO time, slug).

Fixture-only parts: the execution double (`RecordingExecution`) and the `SessionLocations` ports in tests, like the baseline test layer (`SessionExecution` stub). Storage, migrations, event store, projectors, admission and service are real.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| 1. Sibling compile drift | `src/todo.rs` and `tests/todo.rs` (p04.lifecycle) still used event-api v1 (`Tx = dyn Sql`, generic `PublishOptions`). I fixed `src/todo.rs:39` in the working tree only (uncommitted, owner was offline); `tests/todo.rs` still fails to compile | event-api v2 (849d43b09) landed after lifecycle wrote them | p04.lifecycle | `cargo test -p opencode-session-runtime --test todo` compiles |
| 2. Project resolution | `SessionLocations::resolve_project` is a port; no production impl yet | `ProjectV2.resolve` lives in p02 locations | p02 / p91 composition | runtime composition wires locations |
| 3. Legacy `session.updated` | column set follows Drizzle `.set()` undefined-skipping; v1 schema not yet in opencode-schema (`v1.rs` placeholder), so payload is decoded locally | schema p01 WIP | p01 | swap to schema v1 types |

## What I did not check

- Byte-identical JSON of projected `session_message.data` vs a bun-produced database (no TS interop fixture captured).
- `time_updated` on `session_message` updates (baseline Drizzle `$onUpdate` behavior approximated with now()).
- `events()` live durable stream (only `history`/`read_aggregate` paging is tested).
- Replay of recorded Session events through `SqlEventStore::replay_all` (baseline test "reprojects pending inbox input without scheduling execution") was not ported.
- Windows path normalization of `directory`/`path` columns.
- MIME table parity of `mime_guess` vs npm `mime-types` beyond the tested extensions.
