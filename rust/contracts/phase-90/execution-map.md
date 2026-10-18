# Swarm execution map (coordinator-owned)

This is the operating map for the parallel port. Each phase owner reads its plan file in `docs/plans/rustport/`, `rust/AGENTS.md`, `rust/contracts/phase-90/decisions.md` and `rust/contracts/crate-edges.toml` first.

## Phase owners, crates and baseline sources

| Phase | Owns (exclusive authoring) | Primary baseline sources |
| --- | --- | --- |
| 01 | `crates/schema`, `crates/test-support`, `xtask/` (headers, edges, size, inventory, fixtures), `contracts/phase-01/`, `contracts/modularity-reviews.toml`, `contracts/runtime-edges.toml`, `tests/differential/` runner | `packages/schema`, `packages/core/src/id`, `package.json`, `.github/workflows` |
| 02 | `crates/storage-api`, `crates/storage-sqlite`, `crates/event-api`, `crates/event-store`, `crates/config`, `crates/locations`, `contracts/phase-02/` | `packages/core/src/{database,event,config,config.ts,location*.ts,project*,workspace.ts,credential*,state.ts,global.ts}`, `packages/effect-drizzle-sqlite`, `packages/effect-sqlite-node`, `packages/opencode/{migration,src/storage,src/config,src/project}` |
| 03 | `crates/llm-api`, `crates/provider-adapters`, `crates/http-recorder`, `contracts/phase-03/` | `packages/llm`, `packages/core/src/{provider.ts,model.ts,models-dev.ts,catalog.ts,aisdk.ts,plugin/provider*,github-copilot,oauth}`, `packages/http-recorder`, `packages/opencode/src/{provider,auth}` |
| 04 | `crates/session-api`, `crates/session-runtime`, `crates/context`, `contracts/phase-04/` | `packages/core/src/{session.ts,session/,system-context,agent.ts,instruction-context.ts,background-job.ts,question.ts}`, `packages/opencode/src/{session,agent,command}` |
| 05 | `crates/platform-api`, `crates/platform-native`, `crates/permissions`, `crates/tool-api`, `crates/tools`, `contracts/phase-05/` | `packages/core/src/{tool,permission*,filesystem*,file*.ts,process.ts,pty*,shell.ts,git.ts,snapshot.ts,patch.ts,ripgrep*,cross-spawn-spawner.ts,image*}`, `packages/opencode/src/{tool,permission,snapshot,patch,format,worktree,git}` |
| 06 | `crates/protocol`, `crates/server`, `contracts/phase-06/` | `packages/protocol`, `packages/server`, `packages/opencode/src/server`, `packages/core/src/public-event-manifest.ts` |
| 07 | `crates/integration-api`, `crates/integration-adapters`, `crates/codemode`, `bins/compatibility-host`, `contracts/phase-07/` | `packages/opencode/src/{mcp,acp,lsp,plugin}`, `packages/core/src/{plugin*,integration*,skill*,reference*}`, `packages/plugin`, `packages/codemode` |
| 08 | `crates/client`, `crates/sdk-embedded`, `xtask/src/codegen/`, `contracts/phase-08/` | `packages/client`, `packages/sdk`, `packages/sdk-next`, `packages/httpapi-codegen` |
| 09 | `crates/ui-terminal`, `bins/opencode` (CLI), `contracts/phase-09/` | `packages/opencode/src/cli`, `packages/cli`, `packages/tui` |
| 10 | `crates/view-model`, `crates/ui-web`, `contracts/phase-10/` | `packages/app`, `packages/ui`, `packages/session-ui`, `packages/storybook` |
| 11 | `crates/desktop-host`, `bins/desktop`, `contracts/phase-11/` | `packages/desktop` |
| 12 | `crates/hosted-domain/src/console`, `crates/hosted-adapters/src/console`, `bins/hosted` (shared with 13, 12 is crate root owner), `contracts/phase-12/` | `packages/console/*`, `infra/` (console parts) |
| 13 | `crates/hosted-domain/src/{sharing,analytics,sites}`, `crates/hosted-adapters/src/{sharing,analytics,sites}`, `xtask/src/cloud/`, `contracts/phase-13/` | `packages/enterprise`, `packages/function` (sharing/sync), `packages/stats`, `packages/web`, `packages/docs`, `infra/`, `sst.config.ts` |
| 14 | `crates/ecosystem`, `xtask/src/ecosystem/`, `contracts/phase-14/` | `github/`, `packages/function/src/{api.ts,github.ts}` (GitHub parts), `packages/slack`, `sdks/vscode`, `.opencode/tool/` |
| 15 | `crates/observability`, `rust/tests/{acceptance,platform}`, `rust/benchmarks`, `rust/fuzz`, `xtask/src/{quality,distribution}/`, `deny.toml`, `contracts/phase-15/` | `.github/workflows`, `packages/containers`, `nix/`, `install`, `script/`, `packages/core/src/observability*` |

Crate-root files shared by two phases (`hosted-domain/src/lib.rs`, `hosted-adapters/src/lib.rs`, `bins/hosted`): Phase 12 creates them and owns the root, Phase 13 adds its `pub mod` lines.

`xtask/src/main.rs` dispatch: Phase 01 owns it and creates a registration point. Other phases add only their `mod` and one dispatch arm.

## Nesting rules

Phase owners decompose their phase into 2-5 leaf implementers on disjoint files plus one reviewer, using `swarm spawn` with `effort: "low"` for implementers. A leaf may split again only if it owns more than ~3000 lines of baseline behavior. Parents synthesize, integrate and commit-check. Parents do not leave children running after their node completes.

## Order inside each phase

1. API crates first (keep them compiling and committed early, consumers build against them).
2. Real adapters and implementations, with fixture tests captured from the baseline where possible.
3. `contracts/phase-XX/{contract,inventory,status}.md`.
4. Reviewer pass, fixes, final commit.

## Integration owner

The coordinator owns Phase 90 records, `crate-edges.toml`, `crates/runtime`, and Phase 91 composition. Phases 92-94 are run by the coordinator plus reviewers after all parallel phases deliver.
