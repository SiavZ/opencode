# Rust port working agreement

Read this before touching `rust/`. It applies to every agent and human working on the port. The plans live in `docs/plans/rustport/` (start with `README.md`, `architecture.md`, `modularity.md`, `production-gates.md`, `coverage.md`). This file turns those plans into concrete, shared operating rules.

## Ground truth

- The TypeScript product in `packages/` is the behavior baseline. Do not edit it. Read full function bodies, follow producers and consumers, and port observable behavior, including errors, defaults, ordering and edge cases.
- Never stub a capability and call it done. A trait with a fake adapter is a test fixture, not a delivery. If something cannot be finished, leave a typed `Unsupported`/`NotYetPorted` error path that is tested, and record it in your phase's `rust/contracts/phase-XX/status.md` blocker table with the exact missing behavior.
- Production side effects are forbidden: no publishing, no live payments, no emails, no pushing to remotes, no cloud deployments, no writes to the user's real `~/.local/share/opencode` data. Use temp dirs and copies.

## Layout and ownership

- Crates: `rust/crates/<dir>`, package name `opencode-<dir>`, lib `opencode_<dir_with_underscores>`. Binaries: `rust/bins/<name>`. Build tooling: `rust/xtask`. Contract bundles, fixtures and decision records: `rust/contracts/`. Throwaway spikes: `rust/spikes/<phase>-<topic>/` with their own empty `[workspace]` table (excluded from the main workspace).
- Every crate and its owner phase is declared in `rust/contracts/crate-edges.toml`. Only the owner edits a crate's files. Need a change in someone else's crate? DM the owner (or the coordinator) with the exact change. Small, additive fixes to unblock compilation are allowed only after notifying the owner.
- First-party dependency edges must be in the allowlist in `crate-edges.toml`. Adding an edge is a Phase 90 contract change: ask the coordinator.
- `rust/Cargo.toml` `[workspace.dependencies]`: append-only, minimal in-place edits. Never reformat, reorder or remove other entries. Prefer versions already pinned there. Always use `dep.workspace = true` in member manifests.

## Build environment (mandatory, the machine is shared by many agents)

```sh
cd rust
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
export SCCACHE_BASEDIRS="$PWD/target" SCCACHE_CACHE_SIZE=100G   # shares compiled deps across target dirs
export CARGO_TARGET_DIR="$PWD/target/<phase>"   # e.g. target/p04. Children of a phase share their phase's dir (disk budget).
cargo check -p opencode-<crate>      # scope builds and tests to your packages
cargo test -p opencode-<crate>
```

Never run a full-workspace `cargo build`/`cargo test` unless you are the integration owner. Never `cargo clean` someone else's target dir. Do not run `bun install` or modify `node_modules` (it is already installed). TypeScript baseline commands for fixture capture must run from the package directory (`packages/<pkg>`), never from the repo root, with `HOME`/`XDG_*` pointed at a temp dir so the real user data is untouched.

### Shared-workspace hazards (one mistake breaks ~20 agents)

- The workspace globs `crates/*` and `bins/*`. A directory there without a valid `Cargo.toml` breaks every agent's cargo commands. Create a new crate by writing `Cargo.toml` and `src/lib.rs` in the same step, then immediately run `cargo metadata --format-version 1 >/dev/null` and fix any error before doing anything else. Never leave a crate directory empty or half-created.
- After any manifest edit (`Cargo.toml` anywhere), run `cargo metadata --format-version 1 >/dev/null` immediately. A broken manifest must be fixed within the same step.
- Keep committed and on-disk API crates compiling. Make API changes in small compiling steps, other teams build against your working tree.
- If another team's crate does not compile and blocks you, DM its owner with the error. Do not rewrite their code.

## Git rules (shared worktree, branch `rust-port`)

- Commit early and often, only your own paths: `git add <paths> && git commit -m "type(scope): summary" -- <paths>`. Use the conventional types from the repo `AGENTS.md` (`feat`, `fix`, `docs`, `chore`, `refactor`, `test`); scope is the crate or phase, e.g. `feat(storage-sqlite): ...`.
- Never `git add -A`, `git add .`, `git stash`, `git reset`, `git checkout -- <other files>`, `git rebase`, `git push`, or amend commits you did not create. If `index.lock` exists, wait a few seconds and retry.
- `rust/Cargo.lock` may be committed with your change even if it contains other teams' entries.
- Do not create branches or worktrees unless the coordinator approves.

## Code rules

- Rust edition 2024, toolchain pinned in `rust-toolchain.toml`. `cargo fmt` your files. Clippy clean for your crates (`cargo clippy -p <crate> -- -D warnings`).
- `unsafe` is denied workspace-wide. A platform adapter that truly needs it must ask the coordinator and document the safety invariant at each use.
- Library error types use `thiserror` and preserve retryable/cancelled/conflict/validation/permission-denied/not-found distinctions. `anyhow` only in bins, xtask and tests.
- Async: tokio. Every task is owned and joined. Cancellation via `tokio_util::sync::CancellationToken`. Never hold a lock across an unbounded await. Bounded channels with explicit overflow behavior.
- Wire compatibility: field names, casing, optional vs null, numeric precision and ordering match the TypeScript JSON exactly. `serde_json` is built with `preserve_order`.
- Files above 500 physical lines need a modularity decision recorded in `rust/contracts/modularity-reviews.toml` (owner Phase 01). Aim for under 400.
- Tests exercise the real implementation through public boundaries. Do not copy production logic into tests. Label fixture-based tests vs live tests.

## Mandatory file header (every `.rs` file, and scripts)

Every code file starts with module docs in this exact shape. Author-owned fields are Purpose, Scope, Invariants and Contract tests. The block from `Uses:` through the line before `Invariants:` is generator-owned: write it by hand until `cargo xtask headers --fix` exists, then regenerate.

```rust
//! Purpose: persist durable prompt admission and reconcile exact retries. Does not run providers.
//! Scope: repository transaction.
//! Uses:
//! rust/crates/
//! |-- schema/src/session_input.rs
//! `-- storage-api/src/admission.rs
//! External:
//! `-- rusqlite::{Connection, Transaction}
//! Used by:
//! rust/crates/
//! |-- runtime/src/compose.rs
//! `-- storage-sqlite/tests/admission_retry.rs
//! Invariants: admission commits before advisory wake, conflicting reuse fails.
//! Contract tests: storage-sqlite/tests/admission_retry.rs
```

Tree grammar: group entries by their root directory (`rust/crates/`, `rust/bins/`, `rust/xtask/`), each root line ends in `/`, entries are sorted paths relative to that root, `|-- ` for all but the last entry and `` `-- `` for the last. `External:` lists external crate paths, same connector rules. An empty tree is written as a single line `` `-- none `` (for `Used by`: `` `-- none, entrypoint `` or `` `-- none, reserved contract (owner: phase-XX, reason) ``). Uses are direct first-party files: `mod` children, `use crate::/super::/opencode_*` targets resolved to their file, plus runtime edges recorded in `rust/contracts/runtime-edges.toml`. Never put secrets or machine-local paths in headers.

## Contracts and status

Each phase keeps `rust/contracts/phase-XX/`:

- `contract.md`: the public API, wire and lifecycle contract the phase produces, its consumers, and its conformance tests.
- `inventory.md` (or `.toml`): registry-level inventory traced from the baseline with source line ranges and coverage IDs from `docs/plans/rustport/coverage.md`.
- `status.md`: what is implemented, what tests run, what is fixture-only vs live, and a blocker table (`item | missing behavior | why | owner | proof needed`).

Phase 90 (coordinator) owns `rust/contracts/phase-90/` and `crate-edges.toml`. Contract changes after approval go through the coordinator with an impact list.

## Coordination

- Prefer the repo plus typed artifacts over chat. DM a specific owner for conflicts and API questions. Do not broadcast.
- When your work finishes, your final response must state: what you built (paths, commits), tests run and results, what is fixture-only, blockers, and what you did not check.

## Decomposition budget (from 2026-10-03 21:28)

Do not call `swarm expand_node` on a node you were assigned unless it owns more than ~3000 lines of baseline TypeScript AND has clearly disjoint sub-areas. Child nodes cost a dispatch round-trip and wait for a free worker slot (cap 12 live workers). Implement the node yourself, end to end, in committed increments. Nodes whose id already has two dots (e.g. `p04.execution.agents`) must never be expanded further.

## Resuming interrupted work

Workers can be interrupted by provider usage limits. A resumed node starts by running `git status --short` and `git log --oneline -15 -- <your paths>` for its owned paths. Uncommitted files in your paths are a previous worker's progress on the same node: read them, adopt what is correct, finish them, then commit. Do not delete them or restart from scratch. A backup of the 2026-10-03 interrupted state is in `~/.jcode/scratch/salvage-1814/`. Commit at least every ~20 minutes of work so interruptions lose little.
