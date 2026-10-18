# p04.execution.turn: provider-turn runner (llm.ts)

Owner: p04.execution. Commit f743a45fc.

## Built

- `session-runtime/src/runner/turn.rs`: `LlmSessionRunner` (+ `RunnerDeps`) implementing `SessionRunner`. `run(force)`: steer pending, else queue pending; advisory with nothing pending returns before any write; `failInterruptedTools`; outer `shouldRun` loop / inner continuation loop with promotion = steer after the first turn, steer re-check at would-be idle, then one queued input. Compaction transitions loop instead of recursing; a second overflow-compaction transition is an error.
- `runner/turn/attempt.rs`: `runTurnAttempt`. Location check, `Agents::select(session.agent)`, `epoch::initialize`/`prepare`, promotion with cutoff = `epoch::latest_sequence` (queue: one queued then steers), any promotion resets the step to 1, history reload every attempt, max-steps last step, one `LlmClient::stream`, overflow withholding, settlement order exactly as llm.ts (provider failure, interrupt, tool failure, `step.ended` with snapshots and cost 0, hosted-only cleanup).
- `runner/turn/request.rs`: headers (baseline order), `promptCacheKey`, system parts, MAX_STEPS message, `toolChoice: none`.
- `runner/turn/tools.rs`: JoinSet of settle tasks sharing the publisher behind a tokio `Mutex`; cancellation only interrupts `settle`, never a publication.
- `runner/ports.rs`: `LlmClient` re-export, `ToolMaterializer`/`MaterializedTools` + `RegistryMaterializer` over `opencode_tool_api::ToolRegistry`, `SystemContextLoader` + `RegistryContext`, `Compaction` (+ `NoCompaction`), `Snapshots` (+ `NoSnapshots`).
- `runner/mod.rs`: `SessionRunner::run_cancellable(session, force, cancel)` with a default that drops `run` on cancel. `execution.rs` now passes the coordinator token to the runner so interruption settles durable state.

## Tests (FIXTURE provider, real SQLite, real admission/inbox/projectors/tool registry)

`tests/runner_turn.rs` (10), `tests/runner_turn_steps.rs` (6), `tests/runner_turn_tools.rs` (5), support `tests/support/provider.rs`.

## Blockers / gaps

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Compaction port | `SessionCompaction.compactIfNeeded/compactAfterOverflow` production impl | `compaction.rs` is p04.legacy and the V2 compaction is not ported; runner uses `NoCompaction` | p04.legacy | runner tests with real compaction (session-runner.test.ts:1092-1240) |
| Skill/reference guidance | `SkillGuidance.load(agent)`, `ReferenceGuidance.load()` | not ported; `RegistryContext` loads only the registry | p05/p04.context | system parts test with guidance |
| Snapshots | `Snapshot.capture/files` | snapshot service not in session-runtime edges; `NoSnapshots` | p02/p05 | `step.ended` snapshot/files test |
| Declined permission/question | `SettleError::Declined` mapping | tool-api `settle` folds permission errors into model-facing failures; the registry adapter never emits `Declined` | p05 tool-api | halt-on-decline test (V1 parity) |
| Composition | `LocationRunners` impl building `LlmSessionRunner` per Location | runtime crate | runtime/p07 | end-to-end serve test |
