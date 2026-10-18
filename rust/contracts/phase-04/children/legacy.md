# p04.legacy child note

Owner node: `p04.legacy`. Files: `crates/session-runtime/src/{compaction,title,summary}.rs`, `src/legacy/**`, `tests/{compaction,compaction_legacy,title,summary,legacy_prompt,legacy_shell,legacy_command,legacy_policy}.rs`, `tests/support/legacy.rs`. Full mapping: `legacy-inventory.md`.

## Delivery

Commits: `62804f5ed` (prompt/cancel/resolvePromptParts, run state, store, shell, command, share), `a96450cb4` (shell/command/ChildHost tests), `dcfa6c03a` (compaction, title, summary, retry/overflow/usage, legacy compaction), `837672c19` (split MCP resource parts).

| Capability | Rust | Baseline |
| --- | --- | --- |
| Legacy store: V1 `message`/`part` reads, `session.updated` patch setters, `message.*`/`message.part.*` durable events | `legacy/store.rs`, `legacy/projectors.rs` | session.ts 540-936, core projector.ts 260-330 |
| Runner state machine (Idle/Running/Shell/ShellThenRun), busy shell, cancel with `lastAssistant` fallback, `SessionStatus` | `legacy/run_state.rs` | effect/runner.ts, run-state.ts, status.ts |
| `prompt`, `loop`, `cancel`, `createUserMessage`, `resolvePromptParts`, `currentModel`, `getModel`, revert cleanup | `legacy/prompt.rs`, `parts.rs`, `resource.rs` | prompt.ts 144-1079 |
| ChildHost seam (3) | `LegacySession::{prompt_child, cancel, resolve_prompt_parts}`, `prompt::child_reply` | task.ts 200-224 |
| `shell` (durable transcript, TERM=dumb, `shell.env` hook, 3 s kill grace via p05 `ProcessRunner`, abort marker) | `legacy/shell.rs` | prompt.ts 451-592, core shell.ts 166-203 |
| `command` templating (`$N`, `$ARGUMENTS`, `` !`cmd` ``, subtask parts, hooks, `command.executed`) | `legacy/command.rs`, `template.rs` | prompt.ts 1356-1484, command/index.ts 36-44 |
| `share`/`unshare`/auto-share | `legacy/share.rs` | share/session.ts |
| Retry backoff/retryable/policy step, overflow, `getUsage` cost accounting | `legacy/policy.rs` | retry.ts, overflow.ts, session.ts 338-405 |
| Internal V2 auto compaction (`compactIfNeeded`, `compactAfterOverflow`) | `compaction.rs` | core session/compaction.ts |
| Legacy compaction selection, prune, create, replay split, auto-continue | `legacy/compaction.rs` | session/compaction.ts |
| Title generation | `title.rs` | prompt.ts 193-253 |
| File-change summaries | `summary.rs` | summary.ts |

Public V2 `compact` stays `OperationUnavailable` (session-api); only the internal runner path is ported.

## Wiring ChildHost seam (3) (for phase 91)

```rust
async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError> {
    self.legacy.prompt_child(run).await.map_err(|e| match e {
        LegacyError::NotYetPorted(what) => ChildError::NotYetPorted(what),
        LegacyError::Cancelled => ChildError::Cancelled,
        other => ChildError::Failed(other.to_string()),
    })
}
async fn cancel(&self, id: &str) { self.legacy.cancel(id).await }
```

Composition must call `legacy::projectors::register_projectors(store)` once after `projection::register_projectors`, build one `LegacyStore`, one `RunState::new(SessionStatus::new(store), Some(children))` per Location, and inject the ports in `legacy::Deps`.

## Validation

`cargo test -p opencode-session-runtime` (target/p04): all suites pass, including the new 8 suites (17 prompt, 5 shell, 8 command, 5 policy, 7 compaction, 6 legacy compaction, 5 title, 3 summary). `cargo clippy -p opencode-session-runtime --tests -- -D warnings` clean; rustfmt on owned files.

Fixture vs live: SQLite, event store, projectors, run state and the native process runner are real. Agents, provider, legacy loop, prompt tools (Read/MCP/LSP/image/plugins), commands, share remote, snapshots and LLM streams are labeled fixtures. Shell tests spawn `/bin/sh` in a temp dir (unix only).

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| B1 legacy loop | `SessionPrompt.runLoop`, `handleSubtask`, structured output tool, `SessionProcessor`, `LLM.run`, `MessageV2.toModelMessages/filterCompacted`, `SessionTools.resolve`. `LegacyLoop` port; `UnwiredLoop` fails `NotYetPorted` so reply-producing prompts fail typed | legacy processor/LLM bridge not ported; ~2000 TS lines across prompt.ts/processor.ts/llm.ts/message-v2.ts/tools.ts | p04 (new node) with p03 guppy, p05 calf | baseline prompt loop tests against fixture streams |
| B2 legacy compaction summary turn | `SessionCompaction.process` summary turn through the processor; `experimental.session.compacting` hook | depends on B1 | p04 | compaction.test.ts parity |
| B3 legacy Agent service | `Agent.get/list/defaultInfo` adapter and `Agent.generate` | `LegacyAgents` port only; V2 `agents.rs` differs in shape | p04.execution / p91 composition | adapter over `agents.rs` + generate test |
| B4 Command registry | built-in `init`/`review` templates, config/MCP/skill commands | `LegacyCommands` port only; needs config (p07) + MCP + skills | p07 macaque / p05 | command/index.ts state tests |
| B5 system prompt + instructions | `SystemPrompt.provider/environment/skills/mcp`, `Instruction.*` | prompt text assets and skill/MCP/reference services not ported | p04 context or new node | system.test.ts / instruction.test.ts |
| B6 reminders | `SessionReminders.apply`, `Session.plan` | needs plan-mode prompt assets and runtime flags | p04 (with B1) | reminders behavior tests |
| B7 remaining legacy Session ops | `create/fork/list/listGlobal/children/remove/setArchived/setMetadata/setWorkspace/getPart/updatePartDelta`, legacy `revert/unrevert`, cursor paging in `messages` | out of this node's critical path for seam 3 | p04 / p06 routes | session.test.ts parity |
| B8 adapters | Read tool, MCP readResource, LSP symbols, image normalize, plugin triggers, ShareNext, title LLM stream | `PromptTools`, `ShareRemote`, `TitleModel` ports with typed unwired adapters | p05, p07, hosted | composition tests |

## Behavior notes

- Baseline quirk kept: `unshare` publishes `session.updated` without `share`, but `sessionRow` skips undefined columns so `share_url` is not cleared in SQLite (tested in `legacy_prompt.rs`).
- `Snapshot.diffFull(from, to)` maps to p05 `Snapshots::diff(from, to, None, None)`.
- Shell output is collected through `ProcessRunner::run(combine_output)`; the baseline streams intermediate `metadata.output` part updates per chunk. Only the final output is persisted here (open question: streaming API on `ProcessRunner`).
