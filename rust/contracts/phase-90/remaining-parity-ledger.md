# Remaining product parity ledger

Date: October 19, 2026. Owner: parity/bug. This is a completion census, not approval.
The coordinator owns final integration and assigns the next-wave work below.

## What the starting 384 actually measures

The unchanged 430-case capture corpus is replayed by
`crates/server/tests/replay_baseline.rs` against `Services::default()`.
Its original ledger contains 222 `not-mounted`, 140 `not-ported`, 15 `status`,
and 7 `body` cases. There are 46 matches. A service missing from this intentionally
empty host does not establish that the production implementation is missing.
Conversely, a stable expected-failure ledger does not establish product parity.

Exact fixture ID, capture sequence, method, route, operation ID, owner and original
category are in `../phase-01/parity/{api,session,native,integration,provider,cli,composition}.md`.
Those seven reports partition all 430 requirements without overlap. Each records
the immutable original ledger hash and the filename-delimited corpus hash.
Regenerate one report with `cargo xtask parity-census session`, substituting its owner.
The baseline and ledger are never rewritten by this command.

## Strict production public check

```sh
OPENCODE_PARITY_REPORT=/absolute/scratch/path/production-replay.json \
  bash /Users/siavash/.jcode/scratch/opencode-rust-audit/repair-env.sh \
  cargo test --locked --offline -p opencode-runtime --test strict_replay -- --ignored --nocapture
```

`runtime/tests/strict_replay.rs` starts two actual isolated `Process` graphs and
calls `app::router(process.services(), ServerConfig::full_host(..))`, with and
without Basic auth. Each graph owns its own SQLite database, committed Git project,
configuration and shutdown. The project files, modified README, fixture model and
unavailable loopback provider reproduce `phase-06/capture/capture.py`. No credentials
or real HOME are inherited into Process options. All captured cases run in sequence.
The shared production comparator checks status, headers, body order and SSE framing.
There is no expected-failure lookup, blessing, fixture deletion or excluded family.

Ordinary workspace tests keep this expensive differential explicitly ignored.
`cargo xtask fixtures run` and `cargo xtask check` select it by default and pass
`--ignored`; the command fails if it fails or executes zero passing tests.
`--strict` remains an explicit alias. `--conformance` selects only the empty-host
ledger diagnostic. A conformance result is never a product parity result.

The machine report records every case, actual category/status/detail and whether
its request path, query or body contains an unbound setup ID. Such a dependency still fails and
still executes. An unbound ID is evidence of a preceding setup failure, not an
independent verdict that the later operation is absent. Missing services remain
`not-ported`; real wire differences retain `status`, `headers`, `body` or `stream`.

## Phase reconciliation against actual production code

The public checks below are the required observations. Their presence in this table
is not a claim that they passed in this mission. Worker and root reports must supply
the result for the exact source revision. Old contract dates do not define status.

| phase / requirement | actual implementation or gap inspected | implementation owner | public check and completion boundary |
| --- | --- | --- | --- |
| 01 strict gates | `xtask/src/fixtures.rs` now separates production replay from ledger conformance; `parity.rs` enumerates exact original debt | bug | `cargo test --locked --offline -p xtask`; explicit strict Process differential must pass independently |
| 02 storage/event durability | concrete SQLite/event/Location crates exist, despite phase-02 contract still saying adapters in progress | iwazaru | scoped `storage-sqlite`, `event-store`, `locations`, `config` tests plus Process restart HTTP; do not infer completeness from the stale contract |
| 02 configuration composition | Process has isolated environment/config/provider composition; global/config/project routes were absent from empty host | iwazaru + chipmunk | all global/config/project fixture rows in strict replay, malformed config startup, remote config local-protocol tests |
| 03 provider catalog/auth/protocols | `provider-adapters/src` has concrete protocols/catalog/credentials; refresh and runtime OAuth composition are active worker changes | bird | scoped provider tests, controlled loopback auth/refresh and actual model turn; registration alone is not a turn |
| 03 unsupported provider families | `providers/registry.rs::route` still explicitly rejects Cohere v2, gateway v3, GitLab, native API, SAP and arbitrary npm/file factories | bird + chick | per-provider controlled protocol/cassette check and actual runtime route resolution; registry typed-refusal tests prove the gap, not delivery |
| 03 cloud credential modes | `providers/cloud.rs` requires supplied Vertex ADC auth, returns typed unsupported for missing AWS default chain and Mantle SigV4 without bearer | bird | controlled credential-chain/signing tests through actual registry composition; no live cloud credentials or calls |
| 04 durable V2 sessions/context | concrete admission/inbox/coordinator/runner/context modules and tests exist; contract draft does not mean these are missing | cat + iwazaru | admission retry/historical/inbox/runner tests and actual Process projection/provider continuation |
| 04 legacy session operations | `runtime/src/legacy.rs` and new `legacy/*` own real V1 persistence; operation parity changes active | cat | 146 captured api-session + legacy-session cases and dependent SSE through strict Process host; pagination/revert/fork/share must have real lifecycle checks |
| 05 native tools/permissions | concrete filesystem/process/snapshot/permission services exist; HTTP adapters and interactive composition active | chicken | scoped native/permission/tools tests and strict fs/file/find/permission/question/PTY rows; PTY children and ask/reply lifecycle must be joined |
| 05 Windows/Linux native behavior | macOS execution cannot establish target runtime behavior | palmtree + chicken | actual native CI build/runtime observations, kept separate from local macOS proof |
| 06 HTTP registry/wire | routes and real service seams are active in server/protocol; original not-mounted counts are only the empty-host snapshot | bat | strict Process replay, real loopback HTTP/SSE/WebSocket tests, full OpenAPI digest; decoder/error/headers cannot be waived |
| 07 integration runtime | MCP/LSP/plugin/codemode implementation exists under integration-adapters; phase-07 status claiming only scaffolds is stale | chick | controlled stdio/loopback services plus Process integration/reference/skill/command routes and actual tool/context lifecycle |
| 08 typed network client | initial `client/src/lib.rs` exported only errors/descriptor transport; ant is now implementing generated operations and real transports, not yet independently accepted | ant | typed current/legacy operations, native/wasm transport, cancellation, SSE/PTY and operation-specific wire tests must pass |
| 08 embedded host | initial `sdk-embedded/src/embedded.rs::OpenCode::create` unconditionally returned `NotYetPorted(RuntimeServices)`; palmtree is now replacing it with production composition, acceptance pending | palmtree | actual in-process client/router/tool graph with shared permissions and joined disposal; typed-refusal tests cannot count as completion |
| 08 code generation | initially absent; ant is now writing `xtask/src/codegen` for Rust operations from the Protocol registry; CLI dispatch is parity's seam | ant + bug | deterministic safe Rust generation and operation checks; phase-08 Promise/Effect/legacy JS/OpenAPI byte-diff outputs are separate remaining contract requirements |
| 09 CLI command tree | concrete run/models/serve/basic terminal exists; remaining commands, JSON events and flags are active worker work | evergreen | actual built executable per command and failure case, controlled provider, import/export/admin/attach/auth flows; help capture is only help |
| 09 TUI application | app/routes/dialogs/pickers being implemented separately from editor/rendering | herb + ant | end-user PTY workflows, real session navigation/pickers and terminal restoration; renamed basic chat is not full UI |
| 10 web application | `ui-web/src/composer/mod.rs::Composer` is a literal textarea; `routes.rs::AppRoutes` always renders SessionPage; SessionPage only mounts composer | next-wave, root assignment pending | real browser editing/submit/navigation/IME/attachments/drafts workflow, not a successful wasm compile |
| 10 web timeline | `ui-web/src/timeline/mod.rs` exposes only `DEFAULT_ROW_ESTIMATE_PX`, no virtualized timeline | next-wave, root assignment pending | browser session replay, row measurement/windowing/anchoring/tool widgets and scroll behavior |
| 10 shared view model | concrete reducer/projection/session-cache/timeline tests exist under view-model, unlike skeletal browser UI | next-wave, root assignment pending | replay/projection checks plus real browser consumer integration; pure reducer tests cannot certify the UI |
| 11 desktop host | concrete bridge/lifecycle/connections/storage/updater modules and many focused tests exist | palmtree for agreed platform work; remaining next-wave assignment pending | actual desktop shell/sidecar lifecycle and target-specific startup/shutdown, not policy inspection alone |
| 11 native updater install | `desktop-host/src/updater/feed.rs::UnsupportedInstaller::install` always returns Unsupported for native install/relaunch | next-wave, root assignment pending | verified local artifact install/relaunch in isolated application fixture; fetching/signature/staging test is insufficient |
| 12 hosted identity | concrete MySQL/tenant/OAuth issuer code and fixture tests exist, but public `console/{auth,workspace}.rs::routes` each returns Router::new | next-wave, root assignment pending | actual controlled public hosted HTTP login/workspace/key/member lifecycle, not adapter tests alone |
| 12 hosted billing | console billing/inference/quotas/referral domain modules and billing_store/stripe adapters are empty; `console/billing.rs::routes` returns Router::new | next-wave, root assignment pending | local MySQL transaction + controlled Stripe/webhook request tests; no real payments |
| 12 hosted gateway/ops | gateway/limits/formats/support adapters are empty, mail exposes only InviteMailer; `console/{support,zen}.rs::routes` each returns Router::new | next-wave, root assignment pending | controlled public inference/rate-limit/support/mail-capture requests with transaction and cancellation assertions; no live provider calls or outgoing mail |
| 13 sharing/analytics/sites | sharing has actual domain/adapter tests; analytics/sites in both hosted crates are empty namespace modules | next-wave, root assignment pending | public hosted sharing checks plus analytics/site behavior inventory and local routes; sharing tests cannot stand in for the empty products |
| 13 infrastructure plans | xtask cloud generator/golden/contract tests exist and are run by fixtures gate | root follow-up owner pending | plan/golden + independent baseline constructor scan; offline syntax checks where tools exist, never deployment |
| 14 ecosystem editor | `ecosystem/src/editor/mod.rs` is only module documentation, no implementation | next-wave, root assignment pending | actual editor bridge/commands/session workflow and controlled adapter tests |
| 14 GitHub run/triage | `ecosystem/src/github/run/mod.rs` and `triage/mod.rs` are empty namespace files; auth has separate concrete code/tests | next-wave, root assignment pending | controlled event/run/triage workflows; auth tests alone do not implement those modules |
| 14 Slack/session bridge | concrete Slack and session HTTP tests exist | next-wave, root assignment pending | controlled socket/session lifecycle + cancellation, no real Slack messages |
| 15 distribution/acceptance | real archive/installer/acceptance code exists; phase-15 October 3 binary-not-built claim is superseded by phase-09 shipped-artifact evidence | palmtree + iwazaru | exact archive hash, extracted executable workflows, native CI and remote SHA verification by root; no current run inferred from old evidence |
| 90/91 remote workspaces | Process still composes UnsupportedTransport/UnsupportedHost at initial inspection | iwazaru adopted | actual controlled remote HTTP/WebSocket move/sync/recovery/lifecycle checks |
| 90/91 web assets/global composition | root explicitly adopted UiAssets/global/config/remote work | iwazaru + chipmunk | GET / must return real shipped assets, plus full global route strict replay and shutdown checks |

## Ownership census

All captured HTTP rows have an accountable current domain owner and API/bat review.
The non-HTTP rows still labelled next-wave are explicitly not assigned to a current implementation
worker. Client/codegen are now assigned to ant and embedded composition to palmtree; those
assignments are not acceptance. Root acknowledged the remaining unassigned functions and
will reassign workers after the immediate wave. They remain incomplete until implementation
and public checks land.
No recursive workers were created by parity.

## First actual production observation, 11:58 UTC

The real Process differential executed all **430** requirements: **347 matched,
83 failed**. This is a separate production measurement, not the original 384-debt
empty-host ledger. Its gate exited 101 and remained red.

| observed category | count | interpretation |
| --- | ---: | --- |
| match | 347 | recorded wire requirement matched this run |
| not-ported | 24 | actual composition returned a missing-service response |
| not-mounted | 5 | unavailable UI fallback, not evidence of absent domain services |
| status | 14 | actual mounted response differed from the capture |
| body | 38 | actual JSON shape, values, order, list or digest differed |
| headers | 2 | actual SSE or VCS content headers differed |

The independent HTTP Session restart test passed. Real pagination acceptance failed
before reaching cursor assertions: the first noReply message POST returned 500
UnknownError instead of 200. It is not recorded as pagination success. Session/cat
and provider/bird received the failing setup ID and actual response. Cat then exercised
the real Process service directly and isolated `Legacy(Internal("No default model"))`;
configured provider catalog enablement remains bird's seam, not a paging assertion.

The complete machine observation is
`/Users/siavash/.jcode/scratch/production-strict-replay-1158.json`, SHA-256
`5eae2bf2c40cdc6a95df36fdec0a490bbba1959f65fff8788cbf92f754d6cd64`.
The adjacent `.log` preserves the invocation and failures. This snapshot came from
the live shared worktree, not a frozen whole-workspace revision. It cannot certify
subsequent owner changes or another platform.

All failures were sent directly to their owners. Exact failed IDs below are
**observations requiring repair**, never expected failures consumed by the gate.

| family | failed count | owner / exact failed requirement IDs |
| --- | ---: | --- |
| legacy-session | 17 | cat: setup:session.create; setup:session.prompt.noReply; session.list#ok; session.create#ok; session.get#ok; session.update#ok; session.todo#missing-session; session.diff#missing-session; session.messages#ok; session.prompt#ok; session.message#ok; session.message#missing-session; session.fork#ok; session.init#ok; session.unshare#ok; session.revert#ok; session.unrevert#ok |
| api-session | 2 | cat/bat: v2.session.list#ok; v2.session.question.reply#wrong-type |
| negative-auth | 2 | cat: auth:valid:GET /api/session; auth:query-valid:GET /session |
| legacy-auth | 2 | bird: auth.set#ok; auth.remove#ok |
| legacy-provider | 3 | bird: provider.list#ok; provider.auth#ok; provider.oauth.authorize#ok |
| legacy-config | 2 | chipmunk/bird: config.get#ok; config.providers#ok |
| legacy-experimental | 16 | iwazaru/chipmunk/cat: experimental.console.get#ok; experimental.console.listOrgs#ok; experimental.console.switchOrg#ok; tool.list#ok; tool.ids#ok; worktree.list#ok; worktree.remove#ok; worktree.reset#ok; experimental.session.list#ok; experimental.projectCopy.generateName#ok; experimental.workspace.remove#ok; experimental.workspace.warp#ok; v2.projectCopy.create#wrong-type; v2.projectCopy.remove#ok; v2.projectCopy.remove#wrong-type; v2.projectCopy.refresh#ok |
| legacy-find | 2 | chicken: find.text#ok; find.files#ok |
| legacy-file | 1 | chicken/chipmunk: file.list#ok |
| legacy-vcs | 3 | chipmunk/bat: vcs.status#ok; vcs.diff#ok; vcs.diff.raw#ok |
| legacy-command | 1 | chick: command.list#ok |
| legacy-agent | 1 | bug harness: app.agents#ok, capture TMPDIR/cache mismatch identified |
| legacy-skill | 1 | chick: app.skills#ok, missing built-in customize-opencode, not a fabricated filesystem fixture |
| legacy-formatter | 1 | chick: formatter.status#ok |
| legacy-project | 2 | chipmunk: project.update#ok; project.directories#ok |
| legacy-sync | 1 | iwazaru/cat: sync.history.list#ok, upstream noReply setup also failed |
| api-agent | 1 | chipmunk: v2.agent.list#ok |
| api-integration | 7 | chick: v2.integration.list#ok; v2.integration.get#ok; v2.integration.connect.key#ok; v2.integration.connect.oauth#ok; v2.integration.attempt.status#ok; v2.integration.attempt.cancel#ok; v2.integration.attempt.complete#ok |
| api-permission | 1 | bat/chicken: v2.permission.saved.list#ok |
| api-fs | 3 | chipmunk/chicken: v2.fs.read#ok; v2.fs.list#ok; v2.fs.find#ok |
| api-command | 1 | chipmunk/chick: v2.command.list#ok |
| api-skill | 1 | chick: v2.skill.list#ok |
| api-pty | 5 | chicken: v2.pty.list#ok; v2.pty.get#ok; v2.pty.update#ok; v2.pty.connectToken#ok; v2.pty.remove#ok |
| negative-notfound | 4 | iwazaru/bat: notfound:GET /api/does-not-exist; notfound:GET /api/session/x/y/z; notfound:GET /does-not-exist-route; notfound:DELETE /api/health |
| sse | 1 | bat: sse:/api/session/:id/event |
| docs | 2 | bat/iwazaru: doc:/doc; doc:/openapi.json |

The capture sets TMPDIR to its cache directory. The first host used the isolated
helper's default tmp directory, affecting one built-in agent permission pattern.
The harness now sets the exact captured TMPDIR/XDG paths. This was a harness defect,
not an absent agent implementation. A second harness defect left `{SHA}` literal
in request routes such as project.update. Requests now bind it to the actual isolated
repository root commit; response SHA wildcards remain unchanged because blob hashes
and commit hashes differ. Neither observation justifies an adapter fallback.
Every strict case remains included. Subsequent
runs also record resolved request values and elapsed timing to diagnose dependent
message IDs and PTY Location/lifecycle issues without waiving them. ERROR tracing
is enabled only for the isolated test process; no live provider credentials exist.

## Current validation record

- `cargo test --locked --offline -p xtask`: 47 tests passed after codegen dispatch wiring
  (24 binary/cloud/distribution, 18 gates, four parity, one owner partition). The newly
  added codegen integration target was not discovered in that invocation and needs its
  own owner-run check. The parity test pins both original corpus and ledger SHA-256.
- `cargo clippy --locked --offline -p xtask --all-targets -- -D warnings` passed after
  codegen wiring at 11:54 UTC. Scoped whitespace checks passed. Commit `2d9e17dde`
  pins exact strict-test selection, original hashes and codegen entrypoint wiring.
- Commit `47f03d611` contains strict default/conformance separation, original census,
  owner partition and the shared replay fixture-loader seam.
- Production replay executed all 430 and failed honestly as detailed above. Session
  restart passed; pagination failed at the actual noReply admission, before paging.
  Request-only Git binding, exact TMPDIR and ERROR diagnostics were added after that
  snapshot and require the next scoped compile/test run. No lockfile edits or
  expected-failure exceptions were made by parity.
- The source-backed next-wave gaps above are still incomplete. Tests of adjacent
  adapters are not acceptance of their empty public products.
