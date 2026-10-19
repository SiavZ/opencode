# TUI application parity, 2026-10-19

## Scope and verdict

Owner: terminal application worker. This document covers `ui-terminal/src/app.rs`, `routes/mod.rs`, `dialog/mod.rs`, and `bins/opencode/src/cli/commands/terminal.rs` plus its `terminal/` modules and executable `tests/terminal_app/` tests. Editor, timeline projection, markdown/diff/tool widgets and incremental ratatui renderer belong to the separate editor/render worker; see `editor-render-status.md`.

**Full TypeScript TUI parity is not complete.** The application is no longer a basic chat renderer: it owns real navigation, modal workflows, durable services and background execution. Unimplemented interactions below remain release blockers, not expected-failure exceptions. No fake adapters or reduced assertions count as completion.

## Baseline to implementation and evidence

| TypeScript baseline requirement | Rust production path | Observed evidence |
|---|---|---|
| `app.tsx` command palette, Home/session routes, `/new` lazy session creation | `ui-terminal/app.rs`, `routes/mod.rs`; `terminal.rs`, `terminal/actions.rs` | Five application state tests passed; executable lifecycle and empty-picker tests added, validation pending below |
| Agent picker hides hidden/subagent-only agents; model selection uses current catalog; variant default clears selection | `terminal/actions.rs` calls `Process.available_agents`, `available_models`, `default_model`, real `SessionService.switch_agent/switch_model`; typed variant selection | Actual executable selection/response test added; variant outcome acceptance not yet observed |
| Session picker, rename, confirmed delete and fork from full session or selected user message | `dialog/mod.rs`, `terminal/actions.rs` call actual V2 SessionService list/get/rename/delete/fork; fork restores selected user text | State tests prove destructive confirmation defaults to cancel. Executable durable lifecycle test added, pending run |
| Prompt history `component/prompt/history.tsx`: valid JSONL, cap 50, adjacent exact dedup, preserve existing mode/parts | `terminal/history.rs`; editor receives shared persistent inputs while drafts stay per route | Two implementation unit tests added; executable Unicode/paste/restart-history test added, pending run |
| Provider execution continues while switching to Home/session picker | `terminal/execution.rs` owns and joins process SessionExecution tasks by SessionID; real admitted steers and queue input, advisory wake for active drains | Executable navigate-while-running/cancel test added, pending run |
| Stream reasoning/text/tool projection, rather than text-only replay | `terminal/projection.rs` consumes editor worker's strict `Message::from_value`; refreshes actual SessionService context; incremental Renderer emits buffer differences | Editor/render worker's projection/renderer tests passed independently. Executable stream proof pending |
| Real permission asks, reject/once/always replies and saved grant persistence | `terminal/permission.rs` uses same Process Location PermissionService as runner and server, never a UI-local registry | Executable actual read-tool test covers reject, once, always and a second executable invocation using same isolated SQLite state; pending run |
| Real questions, single/multiple selection, custom Unicode answer, review, dismissal | `terminal/question.rs` consumes typed `Process.pending_questions`, replies/rejects same shared Questions waiter | Executable loopback model question-tool tests assert no continuation before review and inspect actual tool-result answers/rejection; pending run |
| Terminal restoration on normal exit, provider error, cancellation | Existing TerminalGuard retained; all execution handles settled before explicit restore; executable fixtures compare exact original termios plus alternate screen, cursor, mouse and bracketed paste states | Existing baseline PTY tests retained; new application PTYs assert exact restoration, pending run |

## Validation ledger

All Rust commands run through the shared pinned `repair-env.sh` with `--locked --offline`, scoped to packages. No installs, manifests, dependencies, unsafe code, real user state, external provider traffic or recursive workers were used.

- `cargo test -p opencode-ui-terminal --test application --locked --offline`: **5 passed** before subsequent additive variants/fork changes.
- Editor/render worker independently reports full UI tests and all-target strict Clippy passing; those are renderer evidence, **not executable app acceptance**.
- Application executable test attempts reached actual production dependency compilation but were successively blocked by concurrent owner compile errors in provider catalog, integration skills, runtime integration/global command adapters. Owners were notified; those errors were not edited outside ownership.
- Latest actual executable test build reached owned code and found only missing root `ProviderID`/`ModelID` reexports. Fixed to public `session_api::ids` namespace. Rerun task `6712652fz6` pending shared target lock as of 11:51 UTC. No application PTY pass is claimed yet.
- Pending final scoped runs: full `terminal_app` executable suite; original `pty::` regressions; bin unit history tests; `cargo clippy -p opencode -p opencode-ui-terminal --all-targets --locked --offline -- -D warnings`.

## Exact remaining interactions

| Item | Missing behavior | Why/owner | Proof needed |
|---|---|---|---|
| TUI configuration and command keymap | Production `tui.json/jsonc` source merging, plugin origins, configured bindings/leader timeout and mode-aware command routing; current application hotkeys are fixed | Application + configuration producer integration | Executable configured rebinding/disabled binding/leader timeout and editor conflict workflows |
| Prompt workflows | Command/file/agent autocomplete, attachment/file/directory/image parts, shell mode, prompt stash, queued prompt editor/reordering and external-editor lifecycle | Application/editor integration; current history preserves old parts but new prompt submits text only | Actual model request attachments, persisted stash/queue edits and suspend/editor restoration |
| Model/provider dialogs | Favorites/recent grouping, persisted `model.json` preferences, provider install/connect/API-key/OAuth/organization flow, variant cycling and unavailable-model recovery | Application + provider host ports | Real controlled auth protocols plus executable picker preference persistence |
| Session picker fidelity | Fuzzy scoring, date grouping, pinned/quick switches, query debounce/pagination, inline rename/delete shortcuts, cross-directory filter toggle and workspace association | Application | Executable ordering/search/pagination and error recovery |
| Timeline/message actions | Live scroll-on-selection; message action dialog with revert/copy/fork; all file parts restored on fork/undo; stepwise redo; exact revert boundary and file diff presentation | Application + session/renderer; real domain ports already used | Durable branch/revert/filesystem and clipboard acceptance, not state-only tests |
| Child/background sessions | First-child/parent/sibling cycling, background task routing and retry state, root-family permission/question aggregation and child-session prompt restrictions | Application + actual task/child domain integration | Executable model task spawning with child ask answered from root |
| Workspace navigation | Create/list/move/session migration, unavailable workspace and retry-action dialogs, workspace file changes | Application + workspace services | Actual placed session movement/recovery workflows |
| Sharing/export/clipboard | Consent/share/unshare, transcript-copy, last-answer-copy, export options/open-without-saving/editor flow | Application; CLI operations do not complete TUI interactions | Controlled share transport, clipboard and isolated file/editor workflows |
| Display preferences/layout | Responsive sidebar/context/todo/LSP/status sections, theme choice, timestamps, thinking collapse/expand, conceal/detail/generic-output/scrollbar/diffwrap controls and persisted settings | Application + renderer | Real executable resize, persisted toggles and interaction screenshots/PTY assertions |
| Mouse and terminal UX | Current binary preserves prior mouse-disabled setup; TS default mouse capture and clickable transcript/tool/modal interactions are not connected. Terminal suspend/title/cursor configuration, attention/sounds and which-key are not application-wired | Application + existing TerminalGuard/platform | PTY suspend/resume and modes, clickable selection, notification fixture boundaries |
| Plugins and integration dialogs | Actual plugin route/slot lifecycle, plugin manager/install, MCP/LSP/skills status/toggle dialogs, custom command contributions and debug/console views | Application + integration host | Real plugin route commands and isolated MCP/LSP lifecycle via executable |
| Error behavior | Prior Rust baseline exits with failure on provider error/cancel and restores exactly. TS stays in-session with error rendering/toast and interruption state; recovery UX remains different | Application, baseline compatibility decision required rather than silently changing existing tests | Preserve existing tests plus explicit new recovery acceptance agreed with coordinator |
| Renderer fidelity | Remaining rich markdown tables/syntax, interactive specialized tool expansion, reasoning metadata/timing, streamed task children and diagnostics | Editor/render worker ledger | See `editor-render-status.md`; generic tool output is not full specialized widget parity |

## Delivery boundaries

State tests and local HTTP/SSE fixtures are clearly identified. PTY suites launch the actual executable with real Process composition, session SQLite/event projection, native tools and permission/question waiters, with isolated HOME/XDG/TMP and loopback provider fixtures. They must pass before their rows above can be marked observed. Broader package/platform parity and packaged builds are coordinator-owned and not certified here.
