# Phase 04 contract: sessions, agents and context

Status: draft v0 (parent p04, 2026-10-03). Seams below are binding for p04 child nodes. Changes go through the p04 parent.

## Crates and ownership

| Crate | Files | Owner node |
| --- | --- | --- |
| `opencode-session-api` | all | p04 (parent) |
| `opencode-session-runtime` | `src/lib.rs`, `Cargo.toml` | p04 (parent). Children may append one dependency line (then run `cargo metadata`). |
| | `src/{admission,inbox,projection,store,service}.rs`, `tests/admission*.rs`, `tests/support/sql.rs` | p04.admission |
| | `src/{coordinator,execution,agents}.rs`, `src/runner/**`, `tests/{coordinator,runner}*.rs`, `tests/support/provider.rs` | p04.execution |
| | `src/{todo,revert,question,background,children}.rs`, `tests/{todo,revert,question,background,children}*.rs` | p04.lifecycle |
| | `src/{compaction,title,summary}.rs`, `src/legacy/**`, `tests/{compaction,title,summary,legacy}*.rs` | p04.legacy |
| `opencode-context` | `src/**` (except `lib.rs` module list), `tests/**` | p04.context |
| `rust/contracts/phase-04/` | all | p04 (parent); children write `children/<name>.md` notes |

New module files inside an owned subtree are fine (e.g. `runner/turn.rs`). New top-level modules: ask the parent to add `pub mod`.

## Upstream APIs (do not invent)

- `opencode-storage-api` (p02, dragon): `Value`, `Row`, `trait Sql` (sync: execute/query/query_row/savepoint), `trait Database` + `DatabaseExt::{transaction(TxOptions, |tx: &dyn Sql| ..), read(..)}`, `StorageError::{Busy, UniqueViolation, ForeignKey, Constraint, NotFound, Cancelled, ..}` with `is_retryable()`.
- `opencode-event-api` (p02): `trait EventStore { type Tx: ?Sized; publish(def, data, PublishOptions), project(def, Projector<Tx>), durable(aggregate, after), read_aggregate(..) -> AggregatePage, .. }`. Use `dyn EventStore<Tx = dyn opencode_storage_api::Sql>`. Projector errors (`ProjectorError`, boxed, downcastable) roll back the publish.
- `opencode-llm-api` (p03, guppy): canonical request, `LLMEvent` stream, errors. Runner consumes it through one `stream(request)` call per provider turn.
- `opencode-tool-api` / `opencode-permissions` (p05, calf), `opencode-integration-api` (p07, macaque).

Until an upstream crate is committed and compiling, code against it as announced and keep the crate compiling by gating that module behind nothing else (do not fork the API). If blocked, DM the owner with the exact trait needed and record the blocker in your child note.

SQL: Rust reads/writes the existing Drizzle tables exactly (`packages/core/src/session/sql.ts`, `packages/core/src/database` migrations): `session`, `session_message`, `session_input`, `session_context_epoch`, `todo`, `event`. No schema forks.

## Internal seams between children (binding signatures, adapt types to storage/event API names)

p04.admission exports from `inbox.rs` (consumed by p04.execution runner):

```rust
/// Any unpromoted input of this delivery for the Session (`SessionInput.hasPending`).
pub fn has_pending(tx: &dyn Sql, session: &SessionId, delivery: Delivery) -> Result<bool, SessionError>;
/// Publish `session.next.prompted` for every pending steer with admitted_seq <= cutoff, ascending. Returns count.
pub async fn promote_steers(events: &dyn SessionEvents, session: &SessionId, cutoff: i64) -> Result<usize, SessionError>;
/// Publish `session.next.prompted` for the oldest pending queue input. Returns whether one was promoted.
pub async fn promote_next_queued(events: &dyn SessionEvents, session: &SessionId) -> Result<bool, SessionError>;
```

p04.admission exports from `store.rs`: `get(session) -> Option<SessionInfo>`, `message(id)`, `context(session)` and `projection.rs` exports the event publish helpers used by runner/lifecycle/legacy (`publish_session_event(type, data)` returning durable seq). The concrete `SessionEvents` facade (wrapping `EventStore` + `Database`) lives in `projection.rs`.

p04.context exports `opencode_context::{source::{SystemContext, Source, Snapshot, combine}, registry::Registry, epoch::{initialize, prepare}, history::{load, entries_for_runner}}` (names follow `packages/core/src/system-context/index.ts` and `session/{history,context-epoch}.ts`). Because `context` may not depend on `session-api`, history returns raw `(id, type, seq, data)` rows; session-runtime decodes them into `Message` with `Message::from_row`.

p04.execution exports `coordinator::RunCoordinator<K>` (generic over drain fn), `execution::LocalSessionExecution` implementing `session_api::operations::SessionExecution`, and `runner::SessionRunner` trait `run(session, force) -> Result<(), SessionError>`. Runner talks to the model through an `LlmClient` port whose `stream` signature is exactly `opencode_llm_api`'s; tools through a `ToolMaterializer` port mirroring `packages/core/src/tool/registry.ts` `materialize/settle` until p05 tool-api lands.

## V2 lifecycle (frozen, AGENTS.md "V2 Session Core")

1. Commit one durable `session_input` row (via `session.next.prompt.admitted` projection) before advisory wake. `resume:false` admits only.
2. Session ID reuse adopts. Prompt message ID reuse succeeds only for identical Session, prompt and delivery. Conflicts fail with `PromptConflict`.
3. Historical projected prompts (visible `session_message` user row with no `session_input` row) lazily synthesize a promoted inbox row during exact retry. See "Historical retry discrepancy".
4. Process-global coordinator keyed by Session ID: explicit resume joins (force=true), advisory wake coalesces into one successor (force=false), different Sessions run concurrently, interrupt targets the active owner and joins cleanup, idle interrupt is a no-op.
5. One `llm.stream(request)` per provider turn, projected history reloaded before each turn.
6. Steers promote as one batch up to a captured cutoff at safe boundaries. One FIFO queue input promotes only at would-be idle, then continuation is reevaluated. Each promoted batch resets the agent step allowance once.
7. Advisory drains only drain eligible inputs. No post-crash continuation recovery.

## Historical retry discrepancy (D4 blocking proof)

Baseline `packages/core/src/session/input.ts:41-81` `admit` only looks up `session_input`. If the message was projected before `session_input` existed (database migrated from a release without the inbox, so a `session_message` user row exists but no inbox row), `admit` publishes `PromptAdmitted`, whose projector (`:83-116`) finds the visible message and dies with `LifecycleConflict`, mapped to `PromptConflictError`. That violates AGENTS.md ("Historical projected prompts lazily synthesize promoted inbox records during exact retry"). The baseline test `packages/core/test/session-prompt.test.ts:446-486` publishes `Prompted` through the current projector, which eagerly writes the inbox row (`:118-168`), so it never exercises the message-only state.

Resolution (Rust): `admit` checks `session_input`, then on miss checks `session_message` for the ID. A visible `user` message with matching Session, prompt (`text/files/agents`) is reconciled by inserting a promoted inbox row (`admitted_seq = promoted_seq = message.seq`, `time_created = message.time_created`, delivery = requested delivery since historical messages carry none) in one transaction and returning it. Different Session or prompt, or a non-user message, fails with `PromptConflict`. Tests: `session-runtime/tests/admission_historical.rs` reproduces the baseline failure mode against the raw table state and proves the repair, including concurrent retries.

## Upstream status log

- 2026-10-03 p02 committed `opencode-storage-api` (be15cbc5e: `Sql`/`SqlExt::savepoint`, `Database`/`DatabaseExt::{transaction(TxOptions, ..), read}`, `Value`/`Row`/`FromValue` with `get_json`, `StorageError` + `is_retryable`/`is_conflict`) and `opencode-event-api` (a0bc3818e: `Definition::durable(type, version, aggregate_field)`, `Payload`, `SerializedEvent`, `EventStore<Tx = dyn Sql>` with `publish(def, data, PublishOptions{commit hook})`, `project(def, Projector)`, `durable`, `read_aggregate(id, after, limit, types)`, replay/claim/remove, `EventError::Projector{source}` + `projector_source::<E>()` to downcast typed projector errors). SQLite and event-store implementations are in progress in p02 children.
