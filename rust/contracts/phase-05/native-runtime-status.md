# Native runtime permissions and PTY implementation evidence

Owner: native/permissions worker. Updated 2026-10-19.

## Implemented boundaries

| Source requirement | Concrete implementation | Observed verification |
| --- | --- | --- |
| `packages/core/src/permission.ts`: pending assertion, explicit reply, configured deny precedence, publication cleanup | Existing `PermissionService` reused by runtime; reply serialization and ownership checks prevent stale published replies settling a reused request ID | `cargo test -p opencode-permissions --test service --locked --offline`: 11 passed, including stale reply, cancellation, duplicate ID, deny, always grants |
| `packages/core/src/permission/saved.ts`: durable project-scoped grants | `runtime/location_permissions.rs` uses existing SQLite permission repository, existing agent resolution and real SessionStore | Composed runtime persistence/restart test added; execution pending latest shared build |
| Real runner authorization, not production headless decline | `location_tools::build_with_permissions` receives the same `Arc<PermissionService>` exposed to root Process, HTTP and TUI. Application registrations use `ToolRegistry::fork` | Real Process/HTTP/provider continuation tests added in `runtime/tests/permissions.rs`; execution pending latest shared build |
| `packages/core/src/question.ts` and `tool/question.ts`: ask, reply, reject, cancellation | `tools/interaction/Questions` is the sole pending service; QuestionTool uses actual permission assertion and awaits that service. `location_questions.rs` publishes through shared Location events | `cargo test -p opencode-tools --test questions --locked --offline`: 2 passed, testing real pending/reply/reject/cancel/shutdown/publication failure. Composed provider test added |
| `packages/core/src/pty.ts`: actual terminal, attach/replay/cursor, resize, write, exit retention and shutdown | `platform-native/pty` uses approved portable-pty, owned process/reader/writer coordinator handles, bounded queues, UTF-16 cursor retention, explicit activation, exit-order retention and joined shutdown | macOS public PTY suite: 3 passed, including canceled creation cleanup. New exit-order retention test added after that run |
| Cross-platform actual terminal behavior | Same public PTY test uses native shell/terminal and verifies real TTY/output/resize/remove/shutdown | Linux aarch64 exact test passed 1/1, reported by platform owner from controlled existing local container. Initial Linux failure exposed shell prompt prefix; deterministic newline-producing command fixed test without weakening native checks |
| Server terminal lifetime | `native_services::NativeProvider` returns a lease-retaining adapter; attachments also retain the same Location lease | Implementation compiled as part of pending composed runtime checks; no separate registry or fake terminal |
| Native filesystem and watcher services | `NativeServices.files` and `.watcher` expose existing NativeFileSystem and NativeWatcher; root/global owner composes HTTP file operations | Existing services reused, no second filesystem policy |
| Additional native search builtins | Actual CoreGlobTool/CoreGrepTool registered with real native Ripgrep process runner. Runtime canonical FileServices resolve/external authorization guard precedes search | Existing domain implementations reused; dedicated composed path acceptance still pending |

## Commands and commits

All verification uses `bash /Users/siavash/.jcode/scratch/opencode-rust-audit/repair-env.sh` and `--locked --offline`, with isolated HOME/XDG/TMP. No package installs or user-data operations.

- Permission ownership fix: `999a5f593`.
- Real native PTY service: `744eefd42`.
- Scoped strict Clippy for permissions, tools and platform-native, all targets with `-D warnings`: passed 2026-10-19 11:56 UTC.
- Latest composed runtime permission + production suites and runtime strict Clippy are queued under the shared audit target. Earlier attempts exposed external in-flight provider/integration/runtime composition errors; they are not counted as passes.

## Explicit remaining scope

- Runtime live provider permission/question continuation and saved grant restart tests must pass before this slice is certified end-to-end.
- Windows execution is not claimed locally. Platform owner wired an explicit nonzero native PTY test in CI; actual Windows result is still required.
- Full webfetch/websearch builtins remain unimplemented in `tools/web`; coordinator requested current shared permission/question slice become green before expanding them.
- Todowrite should reuse existing `session_runtime::todo::Todos`, not add another durable store. Constructor and update API supplied to coordinator.
- V1 native child task hosting is owned by session worker and composes the existing child orchestrator. V2 task-host and native snapshot composition remain coordinator/session ownership, not a duplicate tool runner here.
- Exotic escaped descendants holding a PTY slave after leaving its process group need dedicated adversarial acceptance; ordinary subprocess groups are killed before native reader/writer joins.
