# Legacy capability inventory (p04.legacy)

Every exported function/service member of `packages/opencode/src/session/*.ts`, `packages/opencode/src/command/index.ts` and `packages/opencode/src/agent/agent.ts`, mapped to its Rust home or a blocker. Line ranges are baseline TypeScript at the time of the port (2026-10-04). Status: **ported** (implemented and tested), **partial** (Session-side logic ported, an upstream adapter is a port), **owner** (another node owns it), **blocker** (typed `NotYetPorted` or not started; see `legacy.md` blocker table).

Paths below are relative to `rust/crates/session-runtime/`.

## session/prompt.ts (`SessionPrompt`)

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `Interface`, `Service`, `ops` (TaskPromptOps) | 102-150 | `src/legacy/mod.rs` `LegacySession`; `prompt.rs` `prompt_child`, `child_reply` (ChildHost seam 3) | ported |
| `cancel` | 152-155 | `src/legacy/prompt.rs` `cancel` -> `run_state.rs` `RunState::cancel` | ported |
| `resolvePromptParts` | 157-191 | `src/legacy/parts.rs` `resolve_prompt_parts` | ported |
| `ensureTitle` (title) | 193-253 | `src/title.rs` `Titles::ensure`, `clean_title` | partial (TitleModel port; `UnwiredTitleModel` typed) |
| `handleSubtask` | 255-449 | none | blocker B1 (part of runLoop) |
| `shellImpl` / `shell` | 451-592, 1349-1354 | `src/legacy/shell.rs` `shell`, `shell_args` | ported (live process tests) |
| `getModel` | 594-612 | `src/legacy/prompt.rs` `get_model` | ported |
| `currentModel` | 614-633 | `src/legacy/prompt.rs` `current_model` | ported |
| `createUserMessage` | 635-1050 | `src/legacy/prompt.rs` `create_user_message`, `parts.rs`, `resource.rs` | ported (Read/MCP/LSP/image via `PromptTools` port) |
| `prompt` | 1052-1071 | `src/legacy/prompt.rs` `prompt` | ported |
| `lastAssistant` | 1073-1079 | `src/legacy/prompt.rs` `last_assistant` | ported |
| `runLoop` | 1081-1341 | `ports.rs` `LegacyLoop` (`UnwiredLoop`) | blocker B1 |
| `loop` | 1343-1347 | `src/legacy/prompt.rs` `run` | ported (joins/starts run over `LegacyLoop`) |
| `command` | 1356-1484 | `src/legacy/command.rs` `command`; `template.rs` | ported |
| `PromptInput`, `LoopInput`, `ShellInput`, `CommandInput` | 1494-1562 | `src/legacy/types.rs` | ported |
| `createStructuredOutputTool` | 1565-1590 | none | blocker B1 (runLoop tool set) |
| `bashRegex`/`argsRegex`/`placeholderRegex`/`quoteTrimRegex` | 1592-1596 | `src/legacy/template.rs` | ported |

## session/run-state.ts, status.ts, effect/runner.ts

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `SessionRunState.runner/assertNotBusy/cancel/ensureRunning/startShell` | run-state.ts 52-106 | `src/legacy/run_state.rs` `RunState` | ported |
| `cancelBackgroundJobs` | run-state.ts 111-145 | `JobCanceller` impl for `children::Children::cancel_tree` | ported (owner of tree logic: p04.lifecycle) |
| `Runner.make` state machine | effect/runner.ts 1-217 | `src/legacy/run_state.rs` `Runner` | ported |
| `SessionStatus.get/list/set` | status.ts 30-52 | `src/legacy/run_state.rs` `SessionStatus` | ported |

## session/session.ts (`Session`)

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `isDefaultTitle` | 51-57 | `src/legacy/template.rs` `is_default_title` | ported |
| `fromRow` | 59-118 | `src/legacy/store.rs` `from_row` | ported |
| `toRow` | 120-170 | `src/projection/legacy.rs` (p04.admission `session_row`) | owner |
| schemas (`Info`, inputs, `Event`) | 200-330 | JSON objects in `legacy/types.rs`; wire types p01 | partial |
| `plan` | 331-336 | none | blocker B6 (reminders) |
| `getUsage` | 338-405 | `src/legacy/policy.rs` `get_usage` | ported |
| `BusyError` | 407-409 | `LegacyError::Busy` | ported |
| `createNext`, `create`, `fork`, `list`, `listGlobal`, `children`, `remove`, `setArchived`, `setMetadata`, `setWorkspace`, `diff` | 499-827 | V2 `service.rs` (p04.admission) for create/list; legacy variants not here | blocker B7 |
| `get` | 540-544 | `store.rs` `get` | ported |
| `touch`, `setTitle`, `setAgentModel`, `setPermission`, `setRevert`, `clearRevert`, `setSummary`, `setShare` + `patch` | 731-813 | `store.rs` `patch` and setters | ported (setRevert via `patch`) |
| `messages`, `findMessage` | 828-851, 888-904 | `store.rs` `messages`, `latest`, `find_message` | ported (no cursor paging, loads all) |
| `updateMessage`, `updatePart`, `removeMessage`, `removePart` | 627-643, 853-875 | `store.rs` + `projectors.rs` | ported |
| `getPart`, `updatePartDelta` | 645-665, 877-886 | none (`PART_DELTA` definition only) | blocker B7 |

## session/compaction.ts (legacy) and core compaction

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `PRUNE_MINIMUM`, `PRUNE_PROTECT`, protected tools | 28-31 | `src/legacy/compaction.rs` | ported |
| `serialize`, `summaryText`, `completedCompactions`, `turns`, `splitTurn`, `preserveRecentBudget` | 48-163 | `src/legacy/compaction.rs` | ported |
| `isOverflow` | 203-213 | `src/legacy/policy.rs` `is_overflow` | ported |
| `estimate` | 215-221 | caller-supplied estimator (needs `toModelMessages`) | partial (B1) |
| `select` | 223-269 | `src/legacy/compaction.rs` `select` | ported |
| `prune` | 273-317 | `LegacyCompaction::prune`, `prune_candidates` | ported |
| `process` | 319-557 | `replay_split`, `continue_after`, `continue_text`; summary turn `process()` -> `NotYetPorted` | partial (B2) |
| `create` | 559-583 | `LegacyCompaction::create` | ported |
| core `serializeToolContent`, `buildPrompt`, `make.compactAfterOverflow/compactIfNeeded` | core compaction.ts 88-248 | `src/compaction.rs` | ported (fixture LLM stream) |

## Other session files

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| overflow.ts `usable`, `isOverflow` | 10-34 | `src/legacy/policy.rs` | ported |
| retry.ts constants, `delay`, `retryable`, `policy` | 10-209 | `src/legacy/policy.rs` `delay`, `retryable`, `policy_step` | ported (policy as step fn; caller sleeps) |
| summary.ts `unquoteGitPath`, `computeDiff`, `summarize`, `diff` | 10-146 | `src/summary.rs` | ported (p05 `Snapshots::diff` as `diffFull`) |
| revert.ts `cleanup` | 101-124 | `src/legacy/prompt.rs` `cleanup_revert` | ported |
| revert.ts `revert`, `unrevert` | 38-99 | V2 `src/revert.rs` (p04.lifecycle) covers V2; legacy entry points not here | blocker B7 |
| processor.ts `create` (usage accounting, part streaming) | 98-711 | usage math in `policy.rs`; step-finish session totals in `projectors.rs` | partial (B1) |
| llm.ts `run`, `hasToolCalls` | 85-387 | none | blocker B1 |
| message-v2.ts `toModelMessages*`, `page`, `stream`, `parts`, `get`, `filterCompacted*`, `latest`, `fromError` | 131-740 | `store.rs` reads only | blocker B1 |
| message.ts schemas | 11-148 | JSON values | partial |
| message-error.ts | 4-14 | error names used in `child_reply` / `policy.rs` | partial |
| reminders.ts `apply` | 15-90 | none | blocker B6 |
| system.ts `provider`, `environment`, `skills`, `mcp` | 28-146 | none | blocker B5 |
| instruction.ts `clear`, `systemPaths`, `system`, `find`, `resolve`, `loaded` | 34-229 | none | blocker B5 |
| schema.ts `SessionID`, `MessageID`, `PartID` | 7-26 | `opencode_schema::ids::v1` | owner (p01) |
| todo.ts | 11-72 | `src/todo.rs` | owner (p04.lifecycle) |
| tools.ts `resolve` | 41-590 | none | blocker B1 (p05 tool registry) |

## command/index.ts

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `Event.Executed` | 18-20 | `store.rs` `COMMAND_EXECUTED` | ported |
| `Info` | 22-34 | `ports.rs` `CommandInfo` | ported |
| `hints` | 36-44 | `template.rs` `hints` | ported |
| `Default` | 46-49 | none | blocker B4 |
| `Command.state` (init/review/config/MCP/skill commands), `get`, `list` | 65-171 | `ports.rs` `LegacyCommands` port | blocker B4 |

## agent/agent.ts

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `Info` | 35-56 | `types.rs` `AgentInfo` (fields read by prompt paths) | partial |
| `get`, `list`, `defaultInfo`, `defaultAgent` | 356-367 | `ports.rs` `LegacyAgents` port; V2 registry in `src/agents.rs` (p04.execution) | partial (B3) |
| `generate` | 368-445 | none | blocker B3 |

## share/session.ts

| Baseline | Lines | Rust | Status |
| --- | --- | --- | --- |
| `share`, `unshare` | 26-37 | `src/legacy/share.rs` | ported (`ShareRemote` port; `UnwiredShare` typed) |
| `create` auto-share | 39-46 | `share.rs` `auto_share` | ported |
