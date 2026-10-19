# Phase 01 status

Updated by the Phase 01 parent while child nodes run. Each child appends its rows on completion.

## Implemented

| Item | Path | Commit | Tests | Fixture-only vs live |
| --- | --- | --- | --- | --- |
| JS-compatible scalar values (Finite, Int family, DateTimeUtc, paths, optional helper) | crates/schema/src/values.rs | e16091920, c717d8bc2 | unit: number spelling, int safety, millis truncation/range, optional null rejection | values captured live from bun 1.3.14 at baseline, asserted as constants |
| Branded IDs and prefix checks, generators | crates/schema/src/ids.rs, macros.rs | e16091920 | unit | inferred from source + live probe of `Permission.ID` |
| Event envelope, latest/durable registries | crates/schema/src/event.rs | e16091920 | unit (order, wrong type, null optional, versioning, duplicates) | key order captured live from bun |
| Location, Prompt, LLM tool content, Provider, Model | crates/schema/src/{location,prompt,provider}.rs | e16091920 | unit (key order with spreads, empty model shape) | Model.Api/Ref order captured live |
| Enforced xtask gates, aggregate failure reporting, baseline inventory and native CI | xtask/src, xtask/tests/gates.rs, .github/workflows/rust.yml | 56b5b8a13 and audit12 follow-up | 18 focused gate tests; 11 distribution bin tests; 334 fixture-runner tests | offline versioned fixtures; no live provider calls |

## Audit12 gate repair, 2026-10-19

- `edges` resolves locked offline Cargo graphs independently for the native host, Linux, both macOS architectures, Windows and wasm targets. It checks declared membership, allowlists, normal/build edges, dependency cycles including dev edges, and portable-to-native closure. After root removed stale declarations and allow references, the actual workspace edge gate passed.
- `headers` resolves Rust imports/modules and inverse users, including integration targets, inline modules, renamed imports and explicit module paths. `headers --fix <subdirectory>` limits writes, not validation scope. Final broad generated-header updates are owned by the root coordinator. Missing author metadata in three files was explicitly authorized and documented from source inspection.
- `size` counts every physical Rust source line, including tests/build scripts, and requires current independent review records for exceptions. The initial check identified 17 over-budget files; the independent reviewer owns their disposition. Header regeneration changes exact counts, so reviews must match final source sizes.
- `inventory` compares 554 source declarations against `contracts/phase-01/inventory.toml` and `docs/plans/rustport/coverage.md`, retaining source-line provenance and stable coverage IDs. Enumeration does not approve implementation: statuses remain explicitly unverified or not-ported.
- `fixtures run` validates 136 JSON documents and 430 server baseline cases, then runs actual schema, server replay, provider, CLI-help and cloud golden/contract tests. The measured final run executed 334 passing tests across 32 result blocks, including 13 cloud binary-target tests. Runner failure or zero passing tests fails the gate; fixture blessing is disabled.
- `fixtures run --strict` ran the same suites successfully and then correctly failed with exactly 384 known server incompatibilities in `crates/server/tests/fixtures/replay-ledger.json`. Ordinary conformance checks validate ledger agreement and prominently report this debt. They do not claim full production parity; strict mode rejects it.
- Focused tests executed 18 cases, all passed. Distribution tests executed 11 cases in `--bin xtask distribution::tests`, all passed. `cargo clippy --locked --offline -p xtask --all-targets -- -D warnings` and workflow `actionlint` passed.
- CI now requires gates/conformance, portable wasm compilation, native executable compilation and integration tests on Linux/macOS, real archive verification plus extracted-binary acceptance, and an actual Windows executable build check. The workflow was linted locally, not remotely executed or claimed green. No push was made.

Validation used package-scoped locked/offline Cargo commands from `rust/`, the prescribed shared cache/target settings, and isolated HOME/XDG/TMPDIR. Full port parity and live-provider coverage remain outside this repair's approval.

## Strict production verification, October 19, 2026

This section supersedes the audit12 command semantics above, not its historical run record.

- `fixtures run` and `check` now select the real production Process differential by default.
  `--strict` is an alias. The explicitly ignored `runtime/tests/strict_replay.rs` differential
  is invoked with `--ignored`; failure or zero executed passing tests rejects the gate.
- Empty-host ledger checks are available only as explicit `--conformance` diagnostics.
  Their unchanged 384 recorded mismatches are 222 absent routes, 140 absent services,
  15 status differences and 7 body differences. They are not a production verdict.
- `parity-census [owner]` records every captured requirement, route, operation, domain owner
  and original mismatch category. Seven reports in `parity/` partition all 430 cases.
  Corpus and original ledger SHA-256 hashes make their provenance checkable. No fixture
  or expected-failure entry was deleted, changed or blessed.
- `runtime/tests/strict_replay.rs` composes actual isolated Process services and reuses the
  same comparator as the original server replay. A separate nonignored host acceptance
  test creates a Session through HTTP, shuts down, restarts SQLite and reads it back.
- `phase-90/remaining-parity-ledger.md` reconciles all phase domains against actual source,
  including the missing client/codegen, browser/editor, hosted and remote-workspace work
  outside the 430 HTTP cases. Unassigned next-wave functions were reported to root.
- Parser, corpus census, orphan/duplicate checks and the complete disjoint owner partition
  passed through public xtask APIs. Full production differential and current lint results
  are recorded in the cross-phase ledger after execution, never inferred from these tests.

## In progress (historical swarm children)

| Node | Scope |
| --- | --- |
| p01.schema-session | session, session_message, session_event, session_input, revert, session_aux_event |
| p01.schema-domains | agent, permission, question, pty, project, workspace, integration, filesystem, reference, misc_event |
| p01.schema-v1-manifest | v1 contracts, event manifests |
| p01.deps-test-support | deny/audit/licenses, crates/test-support |
| p01.verify | independent review against the plan |

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| workspace-wide clippy | `cargo clippy -p ...` failed at 11:38 because `crates/provider-adapters/Cargo.toml` (phase 03, uncommitted) did not parse | another team's half-created crate | phase 03 | `cargo metadata` succeeds (it did again at 11:40) |
