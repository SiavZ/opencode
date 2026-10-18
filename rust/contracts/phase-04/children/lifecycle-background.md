# Background job lifecycle port

Owner node: `p04.lifecycle.background`. Baselines: `packages/core/src/background-job.ts:1-365`, `packages/opencode/src/background/job.ts:1-37` (re-export over an instance node, no extra behavior), `packages/core/test/background-job.test.ts`. Consumers: `session-runtime/src/children.rs` (task tool, `cancel_tree`), and later the experimental `session/:id/background` route and run-state cancel.

## Implemented

Adopted the interrupted `session-runtime/src/background.rs` (compile-only commit `9b62360f8`), kept the public API consumed by `children.rs` (`BackgroundJobs`, `JobInfo`, `JobStatus`, `StartInput` including `on_interrupt`, `JobRun`, `WaitResult`, `is_background`, `metadata_str`, `shutdown`) and `todo::now_millis`. Behavior fixes against the baseline:

- **Publish before run.** The first run is spawned while the state lock is held, after the job is built, so it can only observe the published job. The previous version could race on a multi-thread runtime.
- **Pending before extension.** `pending`/`next`/`tail` are updated before the extension task is spawned under the same lock.
- **Generation races.** Every start gets a fresh token. Late settlements from a superseded generation, a finished job or an abandoned job are ignored.
- **Sequential tails and newest output.** Each run waits for the previous tail. Output is replaced only by a higher sequence. Settlement now happens *before* the tail is released, so a failure interrupts queued runs before they can start (the baseline closes the scope after a scheduler hop, so it gives the same observable result).
- **Failure text.** `Err(text)` maps to `error`. A panic maps to `error` with the panic message, like an Effect defect (`errorText(Cause.squash)`). An interrupt-only exit settles `cancelled` with `error: "All fibers interrupted without error"`, the exact text of the baseline `Cause.squash` (recorded with a probe, see below). Explicit `cancel` records no `error`, like the baseline.
- **Wait timeout snapshot.** On timeout `wait` returns the snapshot taken when the wait began (the baseline closes over `job`), not a fresh read.
- **Shutdown = owner scope close.** The baseline leaves the status at `running` when the owner scope closes (its own test asserts it). The previous Rust version settled as `cancelled`. Now `shutdown` interrupts and joins every task (the registry-wide `TaskTracker`), leaves status `running`, and resolves pending waiters with `info: None` instead of hanging forever (in the baseline their fibers are interrupted). The registry stays usable afterwards, like the baseline.
- **Tracked task joining.** Every run is tracked by the job tracker (joined by `cancel`) and the registry tracker (joined by `shutdown`). The previous version reopened a closed per-job tracker, which allowed a `cancel` to return before a concurrent extension was tracked.
- **Wire shape.** `JobInfo` field order now follows the baseline object key order (`metadata` after `started_at`). `list` ties keep insertion order (stable sort over `IndexMap`), like a stable sort over a JS `Map`. Generated IDs use `identifier::create("job", ascending)`.

## Test boundaries

`tests/background.rs` (13) and `tests/background_control.rs` (7), with shared input builders in `tests/background/helpers.rs`, drive only the public API on real tokio runtimes (current-thread, a 2-worker multi-thread runtime, and paused virtual time). No fixtures, no mocks, no copied logic. Coverage: ports of all four baseline tests (100x publish-before-run, 100x pending-before-extension, explicit observation, scope-close keeps `running`), plus duplicate start, tail ordering with out-of-order gate release, failure keeping newest output and dropping queued runs, panic as defect, timeout snapshot, unknown/finished wait, promote hook-once and waiter wakeup, `background: true` at start, never-resolving promotion waits, cancel joining runs plus `on_interrupt` before returning, cancel keeping completed extension output, restart ignoring the old generation, and the exact wire JSON.

Baseline probe (live, not committed): a scratch script run from `packages/core` with `HOME` pointed at a temp dir showed `Effect.interrupt` as `{"status":"cancelled","error":"All fibers interrupted without error"}`, `Effect.die("boom")` as `error: "boom"`, `Effect.die(new Error("defect"))` as `error: "defect"`, and a job interrupted by scope close keeping `status: "running"`.

## Validation log

From `rust/` with `RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3 SCCACHE_BASEDIRS=$PWD/target CARGO_TARGET_DIR=$PWD/target/p04`:

- `cargo test -p opencode-session-runtime --test background --test background_control`: 13 + 7 passed, run 8 times without a flake.
- `cargo test -p opencode-session-runtime`: every test binary in the package passed.
- `cargo clippy -p opencode-session-runtime --all-targets -- -D warnings`: clean (includes the `children.rs` consumer).
- `rustfmt --edition 2024 --check` on the owned files: clean. `src/background.rs` is 499 lines, so no modularity review is needed.

## Blockers and gaps

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| panic text | non-string panic payloads report `Unknown panic`. The baseline would stringify the defect object | Rust panics carry no generic Display | p04.lifecycle.background | none unless a consumer depends on it |
| re-entrant cancel | `cancel(id)` awaited from inside the same job's run or `on_interrupt` hook deadlocks (it joins its own task). The baseline would interrupt itself | tokio joins cannot self-interrupt | p04 consumers | documented in the header. `children.rs` does not do this |
| instance scoping | the baseline opencode node is per-instance (`InstanceState`). Rust exposes a plain `BackgroundJobs` that the composition must create one per instance and `shutdown` on dispose | composition is owned by runtime wiring | p04 runtime compose | compose test that disposes an instance and observes `shutdown` |
| HTTP route | `/experimental/session/:id/background` promote fan-out is not wired | server phase | server owner | route test against `BackgroundJobs::promote` |
