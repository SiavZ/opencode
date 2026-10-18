# Published interfaces for dispatch (coordinator relay)

Interfaces announced by workers that downstream nodes must build on. Dispatch prompts reference this file. Newest first.

## runtime composition root (Phase 91, coordinator), commits dedb72c52, 4c9bae40e

- `opencode_runtime::{Process, ProcessOptions, GlobalPaths, ListenOptions, Listener}`. `Process::start(ProcessOptions::isolated(tmp))` boots the real stack in a temp root (tests); `ProcessOptions::from_process(version, channel)` for the binary. Fields: `database`, `events`, `session` (`LocalSessionService`), `execution`, `locations` (registry), `workspaces`, `projects`. `process.services()` gives server `Services`; `process.serve(listen, cors)` gives a full-host `Listener`; `process.shutdown()` is ordered.
- `opencode serve` is live in `bins/opencode/src/serve.rs` (p09.cli-commands: keep `CommandId::Serve` routed there or call `serve::run` from dispatch; do not duplicate).
- Location bundle: `opencode_runtime::location::LocationBundle { info, runner }`. Domain nodes that produce a Location-scoped service should expose a constructor taking explicit deps; the coordinator adds the field and wiring. See `rust/contracts/phase-91/composition-notes.md` status table.

## native process runner (Phase 05, boar), commit a1bc63167

- `platform_native::process::NativeProcessRunner` implements `platform_api::process::ProcessRunner`: own process group, SIGTERM then grace (default 3s) then SIGKILL of the group, group killed on drop, `run_stream` yields non-empty lines. 9 real-process tests in `platform-native/tests/process.rs`.
- Additive `platform_api::process::RunOptions` fields: `output_sink: Option<mpsc::Sender<Vec<u8>>>` (live chunks with backpressure) and `detached: Option<bool>`. Construct with `..RunOptions::default()`.
- PTY backend is still a placeholder (owned by p05.native-process).

## config (Phase 02, rhino)

- `opencode-config` keeps an internal std-backed fs adapter (`config/src/fs.rs`) until platform-api provides: (1) `read_to_string_safe(path) -> Result<Option<String>>` treating NotFound/PermissionDenied as None (FSUtil.readFileStringSafe), (2) `up(targets, start, stop)` ancestor walk (`packages/core/src/fs-util.ts:168-182`), (3) `glob(dirs, recursive, extensions)` with dot files and symlink following (`{agent,agents}/**/*.md`). Owner of the addition: p05.native-fs. Config switches over once they exist.
- macOS MDM managed preferences use `plutil` via std::process in `config/src/legacy/remote.rs` (`SystemManagedPreferences`). Should move to `ProcessRunner`.
- Remote/org config goes through `config::legacy::RemoteFetcher` with an `UnsupportedRemote` default. Runtime composition must supply an HTTP adapter (GET, Accept json, transient retry).

## server HTTP core (Phase 06, badger), commits 699862e39..453a066ec

- Routers: `opencode_server::app::{router(services, ServerConfig), embedded_router(services), current_router(services, password), router_with(&[&Api], services, config)}`. Serving: `lifecycle::{listen, serve}`. Live socket example in `crates/server/tests/support/mod.rs`. `p06.fixtures` replays against these.
- Streams seam in `handlers/streams.rs` (owned by `p06.server-streams`): keep `pub fn owns(name: &str) -> bool` (dispatcher skips those operations) and `pub fn routes() -> axum::Router<crate::app::AppState>`, replace the bodies. `AppState { services (incl. services::streams), config }`.
- Handlers for every `session.*` operation and `session.messages` dispatch on `Endpoint::name`, need `Middleware::SessionLocation` on sessionID-scoped endpoints, and read payload/query by baseline field names. `p06.current-registry` makes them live by registering the groups.
- Server-side ports for Phase 02 in `crates/server/src/services/ports.rs`: `LocationResolver::resolve(&Location.Ref) -> Result<Location.Info, PortError>`, `SessionPlacement::placement(session_id) -> Option<(directory, workspaceID?)>`, `LegacyWorkspaces { configured_workspace_id, session, workspace, target -> Local{directory} | Remote{url, headers}, is_syncing }`. `opencode-locations` should implement these or publish equivalent traits.
- Blockers: `rust/contracts/phase-06/server-http-status.md`.

## tools file patch engine (Phase 05, sloth), commit "feat(tools): port core and legacy apply_patch engines"

- `opencode_tools::files::patch::{parse(Generation, text) -> Result<Vec<Hunk>, PatchError>, derive(Generation, path, &[UpdateFileChunk], original) -> Result<FileUpdate { content, bom, unified_diff: Option }, PatchError>, join_bom, split_bom, maybe_parse_apply_patch(&[&str]) -> MaybeApplyPatch, unified_diff}`.
- `Hunk::{Add { path, contents }, Delete { path }, Update { path, move_path, chunks }}`. `PatchError.message` equals TS `Error.message`, `.js_string()` gives `"Error: msg"` for legacy `${error}` interpolation. Pure, no filesystem access. `p05.files-apply` consumes it.

## tools file replacers (Phase 05, crab), commits 24363191c, 075a940f1

- `tools::files::replacers::{replace(content, old, new, replace_all) -> Result<String, ToolFailure>, Strategy, candidates(strategy, content, find) -> Vec<String>, trim_diff(diff) -> String}`. UTF-16 Levenshtein, JS whitespace/BOM, JS `replaceAll` dollar substitution. 76-case live TS differential passes.

## platform-api / platform-native / tools (Phase 05), commit 2f747f2bb

- `platform_api::fs::{LocationMutation, ResolveInput { path: String, kind: Option<ResolveKind> }, Target { canonical, resource, external_directory }, PathError}`.
- `platform_native::fs::NativeLocationMutation::new(directory).resolve(input)`. Native fs is otherwise still a skeleton. `p05.native-fs` must preserve the location module.
- `tools::files::{Generation::{Core, Legacy}, FileServices { fs: Arc<dyn FileSystem>, mutation: Arc<dyn FileMutation>, location: Arc<dyn LocationMutation>, directory: PathBuf, worktree: PathBuf }}` with `.new(fs, mutation, location, directory, worktree)`, `.resolve(path, kind)`, `.authorize(target, action, context, metadata)`, `.authorize_external(target, context)`.
- `tools::files::schemas::{input_schema(generation, name), output_schema(generation, name), description(generation, name)}`.

## schema permission (Phase 01, bear, working tree)

- `permission::Ruleset = Vec<Rule>`, `Rule { action: String, resource: String, effect: Effect }`, `Effect::{Allow, Deny, Ask}`, `Request/Asked`, `Replied`, `Source::Tool`, `Reply`, `SavedInfo`, `DEFINITIONS`.

## ui-terminal terminal lifecycle (Phase 09, lion)

- `TerminalGuard::acquire()` / `with_options(TerminalOptions { mouse, kitty_keyboard })`, `suspend()`, `resume()`, `restore()`, `is_active()`, `termination_signal() -> Option<i32>`, global `terminal::restore()`. SIGTERM/SIGHUP restore terminal modes and publish the signal without exiting. `p09.tui-app` must observe it, dispose scoped run/plugins, then print the epilogue.

## context algebra (Phase 04, goat)

- `Source<A>::new(key, loader -> Result<Observation<A>, ContextError>, baseline, update).with_removed`, `A: Serialize + DeserializeOwned + PartialEq + Send + Sync`. `make` erases `A`. Registry `Entry::new(producer -> SystemContext)` with RAII `Registration`. Removal is an empty combine, transient loss is `Observation::Unavailable`.

## session-runtime (Phase 04)

- `SessionRunner` seam for execution children, commit bd7fc9ad0. `SessionError::Cancelled` is the mapping target for storage/event cancellation.

## ecosystem GitHub auth (Phase 14, frog), commit b73e9f981

- `github::auth::handlers::{HttpRequest, HttpResponse, ExchangeService::handle}`, `bridge::FeishuBridge::handle`.
