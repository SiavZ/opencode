# Task-tool child Session lifecycle port

Owner node: `p04.lifecycle.children`. Baselines: `packages/opencode/src/tool/task.ts:98-371`, `packages/opencode/src/session/run-state.ts:111-145` (`cancelBackgroundJobs`), `packages/opencode/src/agent/subagent-permissions.ts`, legacy `Session.createNext/get` (`packages/opencode/src/session/session.ts:499-545`), `MessageV2.get` (`packages/opencode/src/session/message-v2.ts:510-523`).

## Implemented

- `src/children.rs` `Children::run` (TaskTool.execute). The background flag gate comes first, then the depth gate (counts ancestors of the calling Session and fails before any ask or create). Next come the `task` permission ask (skipped on `bypass_agent_check`), agent lookup ("Unknown agent type"), and `task_id` resume (a lookup failure falls back to create). Then child creation with derived permissions plus tool denies deduplicated by (permission, pattern, action), the calling message model/variant (the agent model wins and drops the variant), metadata, and the prompt ops readiness gate ("TaskTool requires promptOps"). After that, `extend` for a running child ("Background task updated"), otherwise `start` with `on_promote` (metadata republish + notifier) and `on_interrupt` (child prompt cancel). For background runs it notifies and returns. For foreground runs it races wait, promotion and abort. Abort cancels the child prompt and the job. A registry shutdown during the join also cancels the child prompt and returns `Cancelled`.
- The metadata `model` key order matches the baseline objects: `{providerID, modelID}` for agent models (`Provider.parseModel`), `{modelID, providerID}` for the calling message.
- The notifier spawns one owned task per job on the `Children` `TaskTracker`. Completed and error produce exactly one synthetic parent prompt (`renderOutput` with "Background task completed/failed: <description>"). Cancelled or shut-down jobs produce none. Injection uses the parent's current agent and falls back to `ctx.agent`. Its failures are ignored, like `Effect.ignore`.
- `cancel_tree` is a port of `cancelBackgroundJobs`: a transitive set over job id, `metadata.sessionId` and `metadata.parentSessionId`, with batches cancelled concurrently and every cancel joined.
- `src/children/types.rs`: wire values, the `ChildHost` port, `derive_subagent_permission`, and `render_output` byte-identical to task.ts. `prompt_child` maps the assistant error (`Subagent failed (task_id: X): msg`), the last failed tool part, and otherwise the last text part. Added `ChildError::{NotFound, Storage}` and `ChildHost::prompt_ready`.
- `src/children/store.rs` (new): production `LegacySessions` over real SQLite. `get` reads `session` and fails with `Session not found: <id>`. `create_child` publishes the legacy `session.created` event so the existing projector writes the row (descending `ses_` id, slug, version, project, directory, path, workspace, parentID, title, agent, permission, zero tokens). `calling_model` reads the `message` table, which fails with "Not an assistant message" for user rows. `ComposedChildHost` combines that with the `ChildOps` port. `UnwiredChildOps` returns typed `NotYetPorted` for the unwired seams.

## Test boundaries

`tests/children.rs` (7) and `tests/children_background.rs` (5) share `tests/children/fixture.rs`. Session rows, child creation through the real event store and projector, and calling message reads are **live** on migrated temp SQLite (`tests/support/sql.rs`). `FixtureOps` (agent lookup, permission ask, tool metadata, child/parent prompt ops) is **fixture-only**, because those producers belong to other nodes. Coverage:

- Exact permission JSON in the stored child row.
- `parent_id`, agent and title on the child row.
- Model/variant inheritance and the agent-model override, with metadata key order.
- Depth gate at 0/1/2 with no ask and no create on failure.
- The background flag gate, unknown agent, missing parent `NotFound`, denied ask, non-assistant caller, bypass, and unknown `task_id`.
- Reply error mapping and resume reusing the child.
- Background start/extend output and a single completion notification with the parent agent and variant.
- Error notification, plus no notification on cancel.
- Foreground abort cancelling the child prompt and the job.
- Promotion returning the background result, republishing metadata, then notifying.
- `cancel_tree` cancelling child and grandchild while a sibling tree keeps running and notifies.
- Typed `NotYetPorted` for unwired agent and prompt ops.

## Validation log

From `rust/` with `RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3 SCCACHE_BASEDIRS=$PWD/target CARGO_TARGET_DIR=$PWD/target/p04`:

- `cargo test -p opencode-session-runtime --test children --test children_background`: 7 + 5 passed, 6 repeated runs, no flakes.
- `cargo test -p opencode-session-runtime`: all package test binaries passed.
- `cargo clippy -p opencode-session-runtime --all-targets -- -D warnings`: clean. `rustfmt --edition 2024 --check` on owned files: clean.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| agent lookup | `Agent.get(name)` -> name, permission rules, model | AgentV2 registry exists in `agents.rs` but is not composed into `ChildOps` | phase 91 composition (producer p04.execution.agents) | runtime `ChildOps::agent` over the registry, plus a test with configured subagents |
| task permission ask | `ctx.ask({permission:"task", patterns:[type], always:["*"], metadata})` | needs the tool context and `opencode-permissions` ask/reply | phase 91 + p05 permissions | denied/always replies through the real permission service |
| legacy prompt ops | `resolvePromptParts`, `ops.prompt` (child and synthetic parent), `ops.cancel` | the legacy SessionPrompt loop is not ported | p04.legacy | child prompt run through the real loop with a fake provider, plus a parent synthetic part |
| tool registration | task tool description/params JSON schema (background variant), `Tool.define` execute, `Effect.orDie` mapping | tool seam not landed | p05.tools-interaction | registry lists `task` with the baseline schema |
| tool metadata | `ctx.metadata` delivery to the running tool part | tool context not landed | p05 / phase 91 | tool part metadata observed after start/promotion |
| run-state cancel | `SessionRunState.cancel` calls `cancel_tree` before the runner cancel | legacy run-state not ported | p04.legacy | session cancel leaves no running child jobs |
| sql.rs header | `tests/support/sql.rs` "Used by" does not list the children tests | owned by p04.admission | p04.admission | header regeneration |
