# Remaining Rust port completion mission, 2026-10-19

Starting point: `dd2499bdd`, clean `rust-port`, already pushed. The user now requests completion of the remaining product-parity gaps, in parallel. The preceding audit repair was a usable slice, not full-product completion. Its passing tests and expected-failure ledger do not excuse any remaining gap.

## Shared rules

- Read `rust/AGENTS.md` and your phase contracts before editing. TypeScript under `packages/` is immutable behavioral ground truth. Read complete producers and consumers, defaults/errors/order/cancellation, not just signatures.
- Shared checkout, no branches/worktrees, no resets/stashes/add-all. Commit only explicitly owned paths, early and in compiling increments, with conventional messages. Only coordinator pushes, explicitly authorized by the user.
- No production/user-data side effects, deployments, payments, emails or package installation. Use isolated HOME/XDG/TMP and controlled loopback/local fixtures. Do not print credential material. For any truly necessary external capability, use the built-in integration discovery first and report constraints honestly. Existing native CI verification may be used without new paid infrastructure or source uploads to a new service.
- Build via `bash /Users/siavash/.jcode/scratch/opencode-rust-audit/repair-env.sh <cargo command>` for the shared pinned offline build/cache environment. Scoped package tests and all-target strict Clippy only; root owns whole-workspace runs. Use `--locked --offline`. Ask coordinator before changing member/workspace manifests or crate edges. Root owns `Cargo.lock`, crate-edges, runtime process/location composition and broad generated headers.
- No placeholder adapter, unconditional permission, fake success, reduced test assertion or generic HTTP fallthrough counts as ported behavior. Never delete a failing fixture or bless an incompatibility as completion. Do not claim full UI parity for a basic renderer.
- Files must stay under 500 physical lines or receive independent concrete review. Extract cohesive boundaries, no compressed formatting or blanket exceptions. Maintain author-owned headers, scoped generator fixes only. Root performs final inverse graph regeneration.
- Each agent must keep todos, implement rather than merely audit, test real public interfaces, record exact baseline requirement -> code -> observed result in a scoped status document, and report unimplemented behavior explicitly. Coordinate additive public seams before dependent work. No recursive spawning.

## Parallel ownership

| worker | exclusive source ownership | main outcome |
|---|---|---|
| API boundary | `crates/server`, `crates/protocol`, phase-06 contracts | all missing legacy route families, exact HTTP/SSE/errors/wire behavior, real service ports |
| Session domain | `crates/session-runtime`, `crates/session-api`, `runtime/src/legacy.rs` and new `runtime/src/legacy/*`, runtime legacy session tests, phase-04 | session pagination/update/delete/revert/fork/share/children/remaining session operations and actual server adapter |
| Native and permissions | `crates/permissions`, `crates/tools`, `crates/platform-native`, `runtime/src/location_tools.rs`, new runtime permission/native adapters | interactive asks/replies/saved grants, broader native tools, public PTY services, watcher/file integration |
| CLI | `bins/opencode` except terminal renderer, phase-09 CLI status | missing commands, advanced run flags, JSON event output, auth/session/admin/import/export/etc via real services |
| TUI application | `crates/ui-terminal/src/app*`, `routes*`, `components*`, dialogs/navigation/new application modules, `bins/opencode/src/cli/commands/terminal.rs`, corresponding tests | actual full-screen app/routes/dialogs/pickers and production entrypoint |
| TUI editor/rendering | `crates/ui-terminal/src/prompt*`, `timeline*`, `editor*`, `render*`, markdown/diff/tool widgets and matching tests | grapheme/display-width editor fidelity, rendering and live interaction; agree module names with TUI application owner first |
| Integrations | `crates/integration-api`, `crates/integration-adapters`, `crates/codemode`, new runtime integration adapters | real MCP/LSP/plugin/skill/reference services and tool/context/lifecycle hooks |
| Providers | `crates/provider-adapters`, provider-api if necessary, new runtime provider auth adapter, phase-03 | catalog refresh/provider auth/config/model-selection/variant gaps, canonical credentials, real controlled protocols |
| Parity gates | `xtask` except root coordination-owned manifests, fixture/coverage metadata after approval, phase-01/status | strict failure census, complete requirement tracking, no expected-failure loophole, independently exercised acceptance |
| Platforms | platform-specific compile/CI/packaging paths agreed with owners, desktop remaining lifecycle differences if verified | actual Windows/Linux verification and real cross-platform corrections, no unsupported claim |
| Coordinator | runtime process/location composition, root manifests/lock/edges, broad headers, phase-90/91 mission and final integration | compose all supplied concrete services, remote config/global services/UI assets/mDNS where required, end-to-end tests, non-force push |

Names/ownership may be refined before touching an unlisted file. DM the coordinator for overlaps and exact seams. Workers do not own each other's files even when a dependency fails compilation.

## Completion evidence

Current owners: API `bat`; sessions `cat`; native/permissions `chicken`; CLI `evergreen`; terminal application `herb`; terminal editor/rendering `ant`; integrations `chick`; providers `bird`; strict parity census `bug`; native platform/CI `palmtree`; global config/projects/files `chipmunk`. Root owns shared Process/Location composition, manifests, lockfile and integration.

The census also covers web/view-model, client/embedded SDK, ecosystem factory/migration, hosted/cloud and remote workspaces. These are not certified by the legacy HTTP recordings or the earlier audit slice; remaining source-backed gaps must be assigned and verified in subsequent work waves.

Start from all 384 strict legacy replay incompatibilities and remaining phase tables. Reconcile them against actual TypeScript behavior and production composition, not documentation age. Missing service cases must be implemented and exercised through a real host, not made green by an empty-host exception. Full CLI/TUI requires genuine executable and PTY workflows. Native Windows/Linux claims require actual build/runtime observations or a clearly recorded external block, not cfg inspection. Finish with whole workspace tests, explicitly enabled local differentials, strict parity, graph/size/inventory, strict Clippy/format, packaged acceptance and remote SHA verification. A missing external prerequisite remains incomplete, never green.
