# Phase 90 operations log (coordinator)

## 2026-10-03 swarm stall

- 14:20-15:04 local: ~32 concurrent Claude Opus workers produced 93 commits (API crates, skeletons, first implementations).
- 15:05 local: the Claude subscription hit its 5-hour usage limit (`429 rate_limit_error`, "resets in 3h 15m"). Workers scheduled server-initiated retries for ~18:20 local.
- 18:20 local: the retries failed with a transient network error (`Failed to send request to Anthropic API`), so 32 child/phase nodes moved to `failed` in the swarm plan. Nothing was lost on disk: 83 uncommitted files from in-flight workers remained in the worktree.
- 21:14 local: uncommitted work backed up to `~/.jcode/scratch/salvage-1814/{tracked.patch,untracked.tgz}` before resuming. 5-hour window back to 0%, 7-day window at 31% (Claude) and 79% (OpenAI).

Corrective policy from here:

- Concurrency is capped at 12 live workers, not 32. The 5-hour window cannot sustain 32 Opus agents.
- Workers use the configured `swarm_model` route where available, spreading load across providers. Claude is reserved for the coordinator and verify/gate nodes.
- Failed nodes resume on the same files. Workers first read `git status` for their paths and adopt the uncommitted work instead of starting over.
- Earlier incident (11:21-12:04): three mis-dispatched sessions (poodle, vole, hare) kept working after `swarm stop` removed them from the swarm. Stop removes membership but does not cancel queued turns. Contained by subscribing to each session on the daemon socket and cancelling turns. The guard ran 45 minutes with zero further activity.

## 2026-10-03 resumed run

- 21:17-21:51: 12-20 workers active (Claude Opus + OpenAI gpt-6.1-sol). 132 commits total on `rust-port` at 21:50. Completed nodes include p02.sqlite (39 tests, live TS interop), p14.github-auth (31 tests), p14.slack (46 ecosystem tests), p05.files-foundation and p05.files-replacers (76-case live TS differential), p04.execution.coordinator, p04.context.algebra (29/29 baseline TS tests, 32 Rust tests), p04.lifecycle.todo, p09.tui-terminal-lifecycle (6 live PTY tests), p11.shell.feed and p11.persistence, p06.server-http, p03.transport, p01.domains-{permission,project,workspace,filesystem}.
- 21:50: OpenAI 7-day window reached 91%. OpenAI workers were stopped after finishing their nodes. New workers run on Claude (5-hour window 26%). `opencode-go` has no credentials on this machine.
- Policy: workers do not decompose further (rust/AGENTS.md "Decomposition budget"). Cross-worker interfaces are relayed in `phase-90/interfaces.md`.

## Throughput budget (22:14)

- Measured burn: Claude 5-hour window went 26% -> 50% between 21:50 and 22:14 with ~9 Opus workers plus the coordinator, about 1% per minute (~0.1%/min per worker). Claude 7-day rose 31% -> 37% between 21:14 and 21:50. OpenAI 7-day is at 93% and is reserved.
- Sustainable rate: a full 5-hour window supports about 3 continuous Opus workers. Running 9 drains it in about 100 minutes and stalls every node (as at 15:05).
- Policy: finished workers are stopped, not reused, until the 5-hour window resets. New workers are spawned only while the window is below 60%, keeping at most 3 live workers once the current wave drains. Paid API routes (Conifer, apiduck, neokens, etc.) are not used without explicit user approval.
- Expected consequence: the remaining scope (about 150 open nodes) takes multiple days of subscription capacity at this rate, not hours.

## Window pause (23:09)

- Claude 5-hour 76% (reset ~02:09 local), 7-day 47%. OpenAI 7-day 93%. 172 commits on `rust-port`.
- Since 21:15 completed: p02.sqlite, p02.repos, p02.events, p03.transport, p03.openai, p03.messages (Anthropic, Gemini, Bedrock), p04.admission, p04.execution.coordinator, p04.context.algebra, p04.context.persistence, p04.lifecycle.todo, p05.files-foundation, p05.files-replacers, p05.files-patch-engine, p06.server-http, p06.server-streams, p06.current-registry (18 groups, 61 operations), p07.mcp, p09.tui-terminal-lifecycle, p09.tui-theme-resolution, p11.shell.feed, p11.persistence, p14.github-auth, p14.slack, p01.domains-{permission,project,workspace,filesystem}.
- Workspace test run 22:48: 925 passed, 1 failed (in-flight p11 lifecycle proxy test), 2 ignored.
- Still running into the pause: tiger (p09.cli-spec), squid (p07.codemode). No new workers until the 5-hour window resets.

## Waves after reset (2026-10-04)

- 02:21 wave 3 (3 workers): p04.execution.model (7d6686dee), p04.execution.messages (820a95044, 10/10 baseline tests), p11.lifecycle.lifecycle (0a7cf2248, proxy test fixed, 36 lifecycle tests). All finished within ~10 minutes using ~3% of the 5-hour window, so the earlier burn estimate was dominated by long-running, context-heavy workers. Revised policy: up to 6 short-lived focused leaf workers per wave while the window is below 60%, stopping each worker as soon as it finishes so context does not accumulate.
- 02:37 wave 4 (6 workers): p04.execution.agents, p04.execution.publish, p04.lifecycle.background, p11.connections, p02.locations, p05.files-read.
