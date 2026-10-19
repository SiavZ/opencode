# Phase 91 composition inputs (coordinator notes)

Traced from baseline `907b3bc51`. These are the composition facts `crates/runtime` must reproduce. Domain owners implement the services, the runtime crate only wires them.

## Location-scoped service graph

Source: `packages/core/src/location-services.ts:42-79` (`locationServices` LayerNode group) and `:83-112` (`buildLocationServiceMap`).

- One service set per `Location.Ref` (`directory`, optional `workspaceID`), created lazily by a keyed map with `idleTimeToLive: 60 minutes` (`LayerMap.make`). Rust: a `LocationRegistry` keyed by Ref with idle eviction and joined teardown (owner of the registry type: Phase 02 `locations`, composition: runtime).
- `Location.node` is replaced by `Location.boundNode(ref)` per entry. Replacements are applied during hoisting so globally tagged dependencies (for example Project) are shared process-wide, not rebuilt per location.
- Global-tagged nodes are hoisted out and provided once (`Node.tags.values.global`). Rust: process-scoped services constructed once in the process root and passed into each location root.
- Members, in declared order (Rust owner in brackets):
  Location [02], Policy [02 config/managed], Config [02], AgentV2 [04], CommandV2 [04], Reference [07], Integration [07], Catalog [03], AISDK [03], PluginV2 [07], PluginInternal [07], ProjectCopy + refresh [02], FileSystemSearch [05], FileSystem [05], Watcher [05], Pty [05], SkillV2 [07], SystemContextRegistry [04], SystemContextBuiltIns [04], LocationMutation [02], FileMutation [05], PermissionV2 [05], ToolOutputStore [05], ToolRegistry + tools [05], Image [05], SkillGuidance [07], ReferenceGuidance [07], SessionTodo [04], QuestionV2 [04], ReadToolFileSystem [05], BuiltInTools [05], SessionRunnerModel [04], Snapshot [05], SessionRunnerLLM [04].
- Booting logs `booting location services` with `directory` and `workspaceID`.

## Process roots

- `SessionExecution` is process-global and Session-ID based (repo `AGENTS.md`, V2 Session Core). It discovers placement via `SessionStore` + `LocationServiceMap.get(session.location)` when a drain starts.
- Legacy CLI entry: `packages/opencode/src/index.ts` (yargs, `scriptName("opencode")`, wrap 100, `--print-logs`, `--log-level` DEBUG/INFO/WARN/ERROR, commands run/generate/console/providers/agent/upgrade/uninstall/models/serve/debug/stats/mcp/github/export/import/attach/tui/acp/web/pr/session/db/plugin). Phase 09 owns the Rust command tree.
- Legacy server: `packages/opencode/src/server/{server.ts,global-lifecycle.ts,init-projectors.ts,projectors.ts,mdns.ts,proxy-util.ts,routes/}`. New server: `packages/server/src/{routes.ts,handlers.ts,auth.ts,cors.ts,location.ts,pty-environment.ts}`. Phase 06 owns both.

## Composition seams to wire (recorded by producers)

- `session_runtime::children::ChildHost` (`crates/session-runtime/src/children/types.rs`, p04.lifecycle.children). `UnwiredChildHost` returns typed `ChildError::NotYetPorted` until runtime injects: (1) agent lookup from `session_runtime::agents` AgentV2 registry (5bf414887); (2) `task` permission ask through `opencode-permissions` ask/reply; (3) legacy `SessionPrompt.prompt`/`cancel`/`resolvePromptParts` from node p04.legacy; (4) task tool registration/description/params from p05.tools-interaction.
- Server ports (`crates/server/src/services/ports.rs`): `LocationResolver`, `SessionPlacement`, `LegacyWorkspaces`, implemented by `opencode-locations` (p02.locations).
- Location registry: `opencode_locations::LocationRegistry<S>` with the per-location bundle built in runtime (see member list above).

## Runtime crate plan

`crates/runtime` (owner: coordinator, Phase 91):

- `process.rs`: process root, global services, shutdown token, joined task tracker.
- `location.rs`: `LocationRoot` construction for a Ref from the domain crates' constructors, 60-minute idle eviction, ordered teardown (reverse of construction).
- `compose.rs`: builds `server` state with real adapters, exposes `serve`, `run` and TUI-attach entrypoints used by `bins/opencode`.
- Tests: boot a location in a temp dir, run a session turn against the fixture provider, verify teardown leaves no tasks or child processes.

## Status (2026-10-04 04:28, commits dedb72c52, 4c9bae40e)

Landed in `crates/runtime`:

- `GlobalPaths` (`global.ts` XDG layout), `Process::start(ProcessOptions)`: SQLite at `Database.path()` under the data dir, `SqlEventStore` + Session projectors, `LocalSessionService`, process-global `LocalSessionExecution` routed through `LocationRegistry` (lease held for the whole drain), `GitProjectResolver`, control-plane `WorkspaceService` with the built-in worktree adapter, ordered `shutdown`.
- Server `Services`: session, locations (registry-backed `Location.Info`), placement, legacy workspace routing, event source. `Process::serve` = full host router.
- `opencode serve` in `bins/opencode/src/serve.rs` (network precedence argv > config.server > defaults, unsecured warning, readiness line, SIGINT/SIGTERM shutdown). Tests: `runtime/tests/process.rs`, `opencode/tests/serve.rs`, serve unit tests.

Location bundle members today: `Location.Info` and the runner seam only. Still to inject as their nodes land (each currently a typed NotYetPorted path):

| member | seam | waiting on |
|---|---|---|
| SessionRunnerLLM | `LocationBundle.runner` (`UnportedRunner`) | p04.execution.turn |
| Snapshot / revert | `SessionProjects::revert` | p05.native-vcs |
| Config per location | bundle field | p02.config `CurrentConfig::load` wiring |
| FileSystem, FileMutation, Watcher, ToolRegistry, BuiltInTools, Permission | bundle fields | p05.native-fs, p05.tools-*, p05.permissions-legacy |
| Catalog, AISDK, provider auth | runner model port | p03.providers, p03.catalog, p03.auth |
| Plugins, MCP, LSP, Skills, References | bundle fields | p07.* |
| ChildHost (task tool) | `session_runtime::children` | p04.legacy, p05.tools-interaction |
| Remote config fetcher | `config::legacy::RemoteFetcher` | runtime HTTP adapter (coordinator) |
| GlobalBus, PTY provider, UI assets | `Services.streams.global/pty`, `Services.ui` | p06.legacy-registry, p05.native-process PTY, p10 |
| mDNS | serve.rs stderr notice | not assigned yet |

## Audit repair status (2026-10-19)

The October 4 table above is historical. The production Location no longer uses `UnportedRunner`.

- `CurrentConfig::load`, native cached/configured catalog and canonical SQLite credential records feed the same `LocationModels` resolver used by `Process::available_models`, CLI execution and the Session runner. Missing cache files are optional, not a first-run error. Credentials stay in transport authorization headers rather than request bodies.
- The Location retains real tool/context registrations and composes `LlmSessionRunner`, native read/write/edit/bash tools, configured agent-rule authorization, built-in system context, native Git snapshots and real summary compaction. Headless `ask` is declined, never silently granted. Interactive permission replies/saved grants, the broader tool families and task-child composition are not claimed here.
- The registry lease covers the whole cancellable drain, including acquisition. Both automatic and overflow compaction waits observe interruption. Live text deltas remain ephemeral; the actual public SSE stream observes them, and interruption flushes accumulated text into terminal durable history before the Session becomes idle.
- The process wires real legacy `LegacyStore` projectors, persisted project identity, placed global/directory events and the required health/session create/list/get/status/message/abort boundaries. Cursor pagination and unregistered legacy route families remain explicitly unsupported, not fabricated.
- Genuine `run`, configured/cached `models`, and a basic no-argument full-screen TUI use the production Process. Actual provider success, failure and in-flight cancellation tests verify terminal restoration. This does not establish full TypeScript TUI or command-family parity.
- Snapshot capture is composed; `SessionProjects::revert`, plugins/MCP/LSP/skills/references, remote configuration fetching, public PTY composition/UI assets and mDNS remain separate unfinished integration work unless their own phase evidence says otherwise.

Current requirement-level checks and extracted-release evidence are recorded in `audit-repair-verification.md`. The strict fixture gate still rejects 384 known empty-host legacy replay incompatibilities; conformance of the stable ledger is not full product parity.
