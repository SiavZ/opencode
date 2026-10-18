# Todo lifecycle port

Owner node: `p04.lifecycle.todo`. Baselines: `packages/core/src/session/todo.ts:1-78`, `packages/opencode/src/session/todo.ts:1-74`, `packages/schema/src/session-todo.ts:1-25`, `packages/core/src/session/sql.ts:100-117`.

## Implemented

- Adopted interrupted `session-runtime/src/todo.rs` rather than replacing it.
- `Todos::update` replaces one Session's rows in one storage-api transaction, assigns array-index positions and timestamps, and only then publishes the schema-owned ephemeral `todo.updated` payload.
- Empty replacement deletes existing rows and still publishes, including a missing Session with no rows. Nonempty replacement for a missing Session fails the existing SQLite foreign key.
- `Todos::get` returns only content/status/priority ordered by position. Status and priority remain arbitrary strings, as in the TypeScript schema.
- Retained `Events`, `publish`, `definition`, `storage_error`, `event_error`, and `now_millis` signatures used by sibling lifecycle modules. Cancellation errors now map to `SessionError::Cancelled` while busy/locked/event retryability is preserved.

## Test boundaries

`session-runtime/tests/todo.rs` calls the actual public `Todos` service through `opencode-storage-sqlite::SqliteDatabase`, on temporary files with the production migrations and foreign keys. It uses two SQLite handles to prove that rows are visible to a separate connection when publication begins. A real SQLite trigger aborts the second insert to prove delete plus partial insert rollback. No storage implementation or schema is duplicated.

The `FixtureEvents` event probe is explicitly fixture-only. It verifies the service's publication definition, wire data, call count, committed state, event failure and cancellation mapping. It cannot prove production event-store subscriber behavior, routing or overflow. Unrelated fixture operations return an explicit typed event error or fail loudly, not pretend to be implemented.

Seven integration tests cover explicit position ordering (reverse unordered reads enabled), ordered replacement and duplicate/unrestricted strings, empty replacement and Session isolation, partial insert rollback with no event, missing-Session foreign keys, postcommit event failure preserving rows, and closed-database failure. A unit test covers helper cancellation/retryability.

## Validation log

- Early package check/test/clippy attempts were blocked by background/revert type errors and a coordinator mutex guard across an await. Exact diagnostics went to owners; coordinator owner fixed its guard scope.
- An isolated, throwaway actual-source probe directly referenced the production module and committed tests, without copied implementation. Its seven SQLite integration tests and one unit test passed, with clean all-targets clippy. This was representative validation only, not the package acceptance result.
- Coordinator subsequently authorized two minimal background/revert compiler fixes. Committed those in `9b62360f8`, preserving the interrupted implementations without additional behavioral edits. Promotion receiver now wraps `Some`; revert validates its fallible branded MessageID before constructing a decode error.
- Actual `cargo check -p opencode-session-runtime` passed after these fixes.
- A transient missing-module error during the context owner's file extraction blocked the first test retry. That owner completed the module set, then actual `cargo test -p opencode-session-runtime --test todo` passed all seven tests.
- Actual `cargo test -p opencode-session-runtime --lib todo::tests::` passed the helper unit test.
- Both `cargo clippy -p opencode-session-runtime --test todo -- -D warnings` and required `cargo clippy -p opencode-session-runtime -- -D warnings` passed. The actual package todo boundary is now validated, not only the isolated probe.
- Scoped `rustfmt --edition 2024 --check` and `git diff --check` passed on owned files.
- `opencode-storage-sqlite` dev dependency was supplied by admission owner in `e4aeca7ea`. Todo implementation/tests committed in `e1bfdf91f`, evidence/header update in `9f5411771`.

Build environment: `RUSTC_WRAPPER=sccache`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=3`, `SCCACHE_BASEDIRS=$PWD/target`, `CARGO_TARGET_DIR=$PWD/target/p04` from `rust/`.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| event-store integration | real ephemeral publication and subscriber receipt | `event-store/src/lib.rs` remains a skeleton; no production concrete adapter available | p02 event-store / dragon | rerun todo update against production event store with subscriber observation |

No new production adapter was invented, and the todo service does not install a fixture event store by default. Production composition must supply the approved `EventStore` implementation. Snapshot, child execution and tools are outside this leaf.

## Not checked

Live TypeScript-versus-Rust capture, real event-store subscriber delivery, concurrent update/event interleaving, process cancellation during the database commit, and end-user HTTP/tool composition are not yet checked. This leaf does not promise an atomic transaction spanning row replacement and event notification, because both TypeScript baselines publish after the row transaction.
