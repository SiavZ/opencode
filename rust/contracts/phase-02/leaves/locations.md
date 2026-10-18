# p02.locations: location registry, project identity, workspace lifecycle

Owner: Phase 02. Crate: `rust/crates/locations` (`opencode-locations`). Normal deps: schema, storage-api,
event-api (plus tokio, tokio-util, tracing, sha1/hex, url, regex, rand). Dev deps: storage-sqlite,
event-store, tempfile. No session/tool/server dependency.

Binding requirements: `locations-requirements.md` (this directory). Baseline:
`packages/core/src/location.ts`, `location-services.ts:42-115`, `location-service-map.ts:7-18`,
`project.ts:65-128`, `workspace.ts`, `packages/opencode/src/project/project.ts:146-310,417-448`,
`packages/opencode/src/control-plane/{workspace.ts,types.ts,adapters/*,workspace-adapter-runtime.ts}`,
`packages/opencode/src/worktree/index.ts` (worktree adapter paths).

## Public API

| Item | Port of | Notes |
| --- | --- | --- |
| `registry::LocationRegistry<S, G = ()>` | `LayerMap.make(.., { idleTimeToLive: "60 minutes" })` | Lazily builds one `S` per `Location.Ref` (directory + optional workspaceID; omitted = implicit-local, keyed separately). `get` returns a `LocationLease` (RcMap reference). |
| `LocationFactory<S, G>` | the per-ref layer closure | Any `Fn(BuildContext<G>) -> impl Future<Output = Result<S>>`. `BuildContext { location, global: Arc<G>, scope }`. |
| `G` (global bundle) | `LayerNode.hoist(.., tags.global)` | Built once by the owner (runtime) and shared by every bundle. Never rebuilt per location. |
| `LocationServices::teardown` | layer finalization | Async, runs once per bundle. |
| `scope::LocationScope` | the bundle's `Scope` | `token()`, `spawn()` (owned tasks), `add_finalizer()` (reverse order). |
| `RegistryOptions { idle_ttl }` | `idleTimeToLive` | `DEFAULT_IDLE_TTL` = 60 min. Zero tears down on last release. |
| `invalidate(ref)`, `shutdown()`, `wait_teardowns()` | `RcMap.invalidate`, scope close | See lifecycle. |
| `project::{ProjectResolver, GitProjectResolver, Resolved { previous, id, directory, vcs }}` | `ProjectV2.resolve/commit` | Phase 04 consumer request: `resolve(&str) -> Result<Resolved, LocationError>`. |
| `project_store::ProjectStore` | legacy `Project.fromDirectory`, `migrateProjectId`, `addSandbox`, `removeSandbox`, `get` | Raw SQL / `storage_api::records` over `project`, `project_directory`, `session`, `workspace` (permission via FK cascade). |
| `git::{GitRunner, SubprocessGit, Repository}` | `Git.repo.discover`, `remote.get`, `history.rootCommits`, raw `git` helper | Narrow trait; `SubprocessGit` spawns `git` with stdin closed. |
| `workspace::WorkspaceService` | `Workspace.Service` | `create`, `list`, `sync_list`, `get`, `remove`, `status`, `is_syncing`, `wait_for_sync`, `synced`, `start_workspace_syncing`, `shutdown`. |
| `workspace::{WorkspaceAdapter, AdapterRegistry}` | `WorkspaceAdapter`, `adapters/index.ts` | Unimplemented methods return typed `Unsupported` (except `list`: empty). |
| `workspace::worktree::WorktreeAdapter` | built-in `worktree` adapter | Real `git worktree` create/list/remove. |
| `workspace::{WorkspaceHost, SyncTransport}` | `Auth.all`, `Session.remove`, HTTP client | Defaults return `Unsupported`; Phase 05/06 inject real ones. |
| `ports::{LocationResolver, SessionPlacement, LegacyWorkspaces}` + impls | phase-90 server ports | See "Server ports". |

## Lifecycle contract (registry)

- Concurrent `get` for the same Ref builds once (tokio `OnceCell`). A failed build caches nothing,
  runs any finalizers it registered, and the next `get` retries.
- `booting location services` is logged (tracing info, fields `directory`, `workspaceID`) once per
  successful build.
- Last lease dropped: an idle timer starts; any `get` before expiry cancels it. After `idle_ttl`
  the entry is removed and torn down.
- Teardown order: cancel the scope token, close and join every task spawned on the scope, call
  `LocationServices::teardown`, then run finalizers last-registered first. `wait_teardowns` and
  `shutdown` join all background teardowns.
- `invalidate`: removes the key immediately (the next `get` builds fresh). If idle, teardown runs
  before `invalidate` returns. If leased, teardown runs when the last lease drops (RcMap semantics).
- `shutdown`: marks closed (later `get` fails `LocationError::Closed`), tears down every bundle,
  leased or not, cancels idle timers and joins everything.

## Project identity

- `resolve(dir)`: discover `.git` upward. Non-git returns `{ id: "global", directory: <fs root>, vcs: None }`.
  Git: id = sha1(`git-remote:` + normalized origin) ?? cached `<common dir>/opencode` ?? first
  sorted root commit ?? `global`. `file:` and non-URL/non-scp remotes are ignored. Directory =
  worktree toplevel. `vcs.store` = common dir, so linked worktrees share the id.
- `from_directory(dir)`: resolve, migrate `previous -> id` in one IMMEDIATE transaction (copy old row
  only if the new one is absent, delete old `project_directory` rows, move sessions keeping
  `time_updated`, move workspaces, delete the old row, which cascades saved permissions like the
  baseline FK), upsert the project row, adopt `global` sessions in that directory, save the
  directory (warn on failure), emit Updated via `on_updated`, then write the id cache (never for `global`).

## Workspace lifecycle

- `create`: validate/generate `wrk` id, `configure`, insert the row, build env
  (`OPENCODE_AUTH_CONTENT`, `OPENCODE_WORKSPACE_ID`, `OPENCODE_EXPERIMENTAL_WORKSPACES=true`, OTEL),
  adapter `create`, then wait up to `timeout` (5 s) for status `connected|error` while starting sync.
  Not atomic, like the baseline: any failure after the insert returns
  `LocationError::CreateFailed { workspace_id, source }` with the row left in place for `remove`.
- `remove`: remove top-level sessions via the host (`NotFound` ignored), stop sync, adapter `remove`
  (failure logged, not returned), delete the row.
- Sync: local targets set `connected`/`error` by directory existence. Remote targets run one owned
  loop per workspace: connect SSE, `/sync/history` with the local `{ aggregateID: seq }` fence,
  replay contiguous runs with `replay_all` (`publish: true`, `ownerID = workspace id`), forward live
  events (sync events through `replay`, heartbeats dropped, others re-emitted on `WorkspaceBus`),
  reconnect with backoff `min(120 s, 1 s * 2^attempt)`. `shutdown` cancels and joins all loops.

## Server ports

`opencode-server` already depends on this crate (crate-edges.toml), so the port traits are published
here with the same shapes as `crates/server/src/services/ports.rs`:
`ProjectLocationResolver` and `impl LocationResolver for LocationRegistry<S: HasLocationInfo>`,
`SqlSessionPlacement`, `WorkspaceRouting` (`LegacyWorkspaces`). The server or runtime wraps them in
its own port traits with one-line delegations (mapping `LocationError` to `PortError` and
`workspace::Info`/`Target` to the server's `WorkspaceInfo`/`WorkspaceTarget`).

## Tests (all live: real git in temp dirs, migrated in-memory SQLite, real SqlEventStore)

| File | Covers |
| --- | --- |
| `tests/registry.rs` | concurrent single build, global sharing, implicit-local keys, 60 min idle eviction on a paused clock with reuse resetting the timer, leased never evicted, invalidate during active work (task cancelled and joined before teardown, fresh bundle built meanwhile), idle invalidate, shutdown, failed build |
| `tests/registry_log.rs` | boot log fields (own binary to avoid tracing interest races) |
| `tests/project.rs` | non-git fallback, precedence remote > cache > root commit, `file:` remote ignored, unborn repo, linked worktree, `Location.Info` JSON, `fromDirectory` migration of sessions/workspaces/permissions/directories + cache write, migrate no-ops, migrate into existing row, sandboxes |
| `tests/workspace_lifecycle.rs` | worktree adapter create/list/remove with real git, `syncList` adoption, non-git/missing-context errors, injected host create failure + cleanup, injected host remove failure, session removal ordering, typed Unsupported defaults, flag off + create timeout, routing ports |
| `tests/workspace_sync.rs` | history `replay_all` + live `replay` with ownership, bus forwarding, `waitForSync` resolve/timeout/abort, SSE decoder |

The remote `SyncTransport` and failing adapters are test doubles standing in for Phase 05/06 hosts.

## Blockers / not ported

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| remote sync transport | real HTTP GET `/global/event` SSE and POST `/sync/history` | HTTP client owned by server/client phases | Phase 06 | `SyncTransport` impl + live test against a server |
| `sessionWarp` | move a session between workspaces (claim, patch copy, remote steal) | needs `EventStore.claim`, session prompt cancel, VCS diff/apply | Phase 04/05/06 | port + test |
| `runInWorkspace` | proxy local/remote calls into a workspace | needs `InstanceStore.provide` and HTTP | Phase 06 | port + test |
| auth env | `OPENCODE_AUTH_CONTENT` from `Auth.all` | provider auth owned elsewhere | Phase 03/06 | inject `WorkspaceHost` |
| session removal | `Session.remove` in `Workspace.remove` | session service | Phase 04 | inject `WorkspaceHost` |
| worktree start scripts | `runStartScripts` (project `commands.start`, extra) and `InstanceStore.load` bootstrap after `reset --hard` | needs instance store and process runner | Phase 05/91 | port + test |
| worktree `reset` / `create` with names | `Worktree.reset`, named create, branch candidates (`opencode/<name>`) | not on the adapter path | Phase 05 | port + test |
| icon discovery | `Project.discover` favicon scan | flag-gated (`experimentalIconDiscovery`) | Phase 02 follow-up | port + test |
| GlobalBus | `WorkspaceBus` is process-local; wiring to the real global event bus | bus owned by runtime | Phase 91 | runtime composition |
