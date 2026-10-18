# Phase 01 status

Updated by the Phase 01 parent while child nodes run. Each child appends its rows on completion.

## Implemented

| Item | Path | Commit | Tests | Fixture-only vs live |
| --- | --- | --- | --- | --- |
| JS-compatible scalar values (Finite, Int family, DateTimeUtc, paths, optional helper) | crates/schema/src/values.rs | e16091920, c717d8bc2 | unit: number spelling, int safety, millis truncation/range, optional null rejection | values captured live from bun 1.3.14 at baseline, asserted as constants |
| Branded IDs and prefix checks, generators | crates/schema/src/ids.rs, macros.rs | e16091920 | unit | inferred from source + live probe of `Permission.ID` |
| Event envelope, latest/durable registries | crates/schema/src/event.rs | e16091920 | unit (order, wrong type, null optional, versioning, duplicates) | key order captured live from bun |
| Location, Prompt, LLM tool content, Provider, Model | crates/schema/src/{location,prompt,provider}.rs | e16091920 | unit (key order with spreads, empty model shape) | Model.Api/Ref order captured live |
| xtask dispatch, lib layout, `check` aggregator | xtask/src/{main,lib,check,workspace}.rs | 90f970ae9 | none yet (gates are stubs) | n/a |

## In progress (swarm children)

| Node | Scope |
| --- | --- |
| p01.schema-session | session, session_message, session_event, session_input, revert, session_aux_event |
| p01.schema-domains | agent, permission, question, pty, project, workspace, integration, filesystem, reference, misc_event |
| p01.schema-v1-manifest | v1 contracts, event manifests |
| p01.xtask-graph | headers, size, edges |
| p01.xtask-inventory-fixtures | inventory, fixtures, baseline fixture capture |
| p01.deps-test-support | deny/audit/licenses, crates/test-support |
| p01.verify | independent review against the plan |

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| workspace-wide clippy | `cargo clippy -p ...` failed at 11:38 because `crates/provider-adapters/Cargo.toml` (phase 03, uncommitted) did not parse | another team's half-created crate | phase 03 | `cargo metadata` succeeds (it did again at 11:40) |
