# Phase 11: Updater Controller Spike

## Objective
Port the desktop auto-updater state machine, persistence, and subscription management from TypeScript to Rust, ensuring exact behavioral parity with the `packages/desktop/src/main/updater-*.ts` baseline.

## Scope
- `rust/crates/desktop-host/src/updater/controller.rs`
- `rust/crates/desktop-host/src/updater/state.rs`
- `rust/crates/desktop-host/src/updater/store.rs`
- `rust/crates/desktop-host/src/updater/subscriptions.rs`
- `rust/crates/desktop-host/src/updater/signature.rs`
- Corresponding test files in `rust/crates/desktop-host/tests/`

## Key Parity Requirements
1. **Coalesced Checks**: Concurrent `check()` calls must join the same in-flight check future.
2. **Dropped Callers**: A dropped caller must not strand the state machine in `checking`; the shared future ensures the task completes and clears the pending state.
3. **Error Recovery**: A failed check transitions to `error`, and the next check runs from scratch. A failed install transitions back to `ready` and surfaces the error.
4. **Wire Shape**: `UpdaterState` JSON serialization must exactly match the TS union (`status` discriminant, kebab-case `up-to-date`, omitted `percent` when absent).
5. **Persisted Ready/Channel**: The file store must preserve the `ready` target and `channel`, handling corrupt files gracefully like `electron-store`.
6. **Subscriptions Disposal**: Replacing a subscription for the same ID must dispose the previous one before registering the new one.

## Testing Strategy
- Real public implementation tests, not copied logic.
- Mock backends and persistence for `controller.rs`.
- Tempdir-based file I/O tests for `store.rs`.
- In-memory map tests for `subscriptions.rs`.

## Blockers / Notes
- None currently. The port is complete and covered by unit tests.
