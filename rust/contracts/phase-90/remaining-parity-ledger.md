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
its request route contains an unbound setup ID. Such a dependency still fails and
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
| 03 unsupported provider families | phase-03 provider blockers name Cohere, gateway v3, GitLab, dynamic JS factories, cloud auth/signing and provider extras; each needs source/test reconciliation by owner | bird + chick | per-provider controlled protocol/cassette check and runtime credential resolution, not a generic compatible endpoint assertion |
| 04 durable V2 sessions/context | concrete admission/inbox/coordinator/runner/context modules and tests exist; contract draft does not mean these are missing | cat + iwazaru | admission retry/historical/inbox/runner tests and actual Process projection/provider continuation |
| 04 legacy session operations | `runtime/src/legacy.rs` and new `legacy/*` own real V1 persistence; operation parity changes active | cat | 146 captured api-session + legacy-session cases and dependent SSE through strict Process host; pagination/revert/fork/share must have real lifecycle checks |
| 05 native tools/permissions | concrete filesystem/process/snapshot/permission services exist; HTTP adapters and interactive composition active | chicken | scoped native/permission/tools tests and strict fs/file/find/permission/question/PTY rows; PTY children and ask/reply lifecycle must be joined |
| 05 Windows/Linux native behavior | macOS execution cannot establish target runtime behavior | palmtree + chicken | actual native CI build/runtime observations, kept separate from local macOS proof |
| 06 HTTP registry/wire | routes and real service seams are active in server/protocol; original not-mounted counts are only the empty-host snapshot | bat | strict Process replay, real loopback HTTP/SSE/WebSocket tests, full OpenAPI digest; decoder/error/headers cannot be waived |
| 07 integration runtime | MCP/LSP/plugin/codemode implementation exists under integration-adapters; phase-07 status claiming only scaffolds is stale | chick | controlled stdio/loopback services plus Process integration/reference/skill/command routes and actual tool/context lifecycle |
| 08 typed network client | `client/src/lib.rs` exports only errors and transport; `transport/mod.rs` implements descriptor/query serialization, no network adapters or operation modules | next-wave, root assignment pending | typed current/legacy operations, native/wasm transport, cancellation, SSE/PTY and operation-specific wire tests are absent acceptance requirements |
| 08 embedded host | `sdk-embedded/src/embedded.rs::OpenCode::create` unconditionally returns `NotYetPorted(RuntimeServices)` despite current Process implementation | next-wave, root assignment pending | actual in-process client/router/tool graph with shared permissions and joined disposal; existing typed-refusal test proves incompleteness |
| 08 code generation | phase-08 contract specifies `xtask codegen`, but `xtask/src/codegen` and command dispatch are absent | next-wave, root assignment pending | byte-diff current/Effect/legacy SDK/OpenAPI outputs, deterministic rerun, safe manifest writer; no fixture blessing |
| 09 CLI command tree | concrete run/models/serve/basic terminal exists; remaining commands, JSON events and flags are active worker work | evergreen | actual built executable per command and failure case, controlled provider, import/export/admin/attach/auth flows; help capture is only help |
| 09 TUI application | app/routes/dialogs/pickers being implemented separately from editor/rendering | herb + ant | end-user PTY workflows, real session navigation/pickers and terminal restoration; renamed basic chat is not full UI |
| 10 web application | `ui-web/src/composer/mod.rs::Composer` is a literal textarea; `routes.rs::AppRoutes` always renders SessionPage; SessionPage only mounts composer | next-wave, root assignment pending | real browser editing/submit/navigation/IME/attachments/drafts workflow, not a successful wasm compile |
| 10 web timeline | `ui-web/src/timeline/mod.rs` exposes only `DEFAULT_ROW_ESTIMATE_PX`, no virtualized timeline | next-wave, root assignment pending | browser session replay, row measurement/windowing/anchoring/tool widgets and scroll behavior |
| 10 shared view model | concrete reducer/projection/session-cache/timeline tests exist under view-model, unlike skeletal browser UI | next-wave, root assignment pending | replay/projection checks plus real browser consumer integration; pure reducer tests cannot certify the UI |
| 11 desktop host | concrete bridge/lifecycle/connections/storage/updater modules and many focused tests exist | palmtree for agreed platform work; remaining next-wave assignment pending | actual desktop shell/sidecar lifecycle and target-specific startup/shutdown, not policy inspection alone |
| 11 native updater install | `desktop-host/src/updater/feed.rs::UnsupportedInstaller::install` always returns Unsupported for native install/relaunch | next-wave, root assignment pending | verified local artifact install/relaunch in isolated application fixture; fetching/signature/staging test is insufficient |
| 12 hosted identity | concrete MySQL/tenant/OAuth issuer code and fixture tests exist | next-wave, root assignment pending | explicit local MySQL and controlled OAuth tests; unavailable DB is an external blocker, never skipped completion |
| 12 hosted billing | `hosted-adapters/src/console/billing_store.rs` is an empty module; `bins/hosted/src/console/billing.rs::routes` returns Router::new | next-wave, root assignment pending | local MySQL transaction + controlled Stripe/webhook request tests; no real payments |
| 12 hosted gateway/ops | formats/gateway/limits/stripe/mail/support modules require next-wave function-level review, no completion inferred from module names | next-wave, root assignment pending | controlled protocol/limits/mail capture with transactional assertions; no live inference or outgoing mail |
| 13 sharing/analytics/sites | source directories exist in hosted crates; this HTTP corpus does not cover those products | next-wave, root assignment pending | local public hosted route/domain tests and exact baseline inventory, independent of opencode serve routes |
| 13 infrastructure plans | xtask cloud generator/golden/contract tests exist and are run by fixtures gate | root follow-up owner pending | plan/golden + independent baseline constructor scan; offline syntax checks where tools exist, never deployment |
| 14 ecosystem editor | `ecosystem/src/editor/mod.rs` is only module documentation, no implementation | next-wave, root assignment pending | actual editor bridge/commands/session workflow and controlled adapter tests |
| 14 GitHub run/triage | `ecosystem/src/github/run/mod.rs` and `triage/mod.rs` are empty namespace files; auth has separate concrete code/tests | next-wave, root assignment pending | controlled event/run/triage workflows; auth tests alone do not implement those modules |
| 14 Slack/session bridge | concrete Slack and session HTTP tests exist | next-wave, root assignment pending | controlled socket/session lifecycle + cancellation, no real Slack messages |
| 15 distribution/acceptance | real archive/installer/acceptance code exists; phase-15 October 3 binary-not-built claim is superseded by phase-09 shipped-artifact evidence | palmtree + iwazaru | exact archive hash, extracted executable workflows, native CI and remote SHA verification by root; no current run inferred from old evidence |
| 90/91 remote workspaces | Process still composes UnsupportedTransport/UnsupportedHost at initial inspection | iwazaru adopted | actual controlled remote HTTP/WebSocket move/sync/recovery/lifecycle checks |
| 90/91 web assets/global composition | root explicitly adopted UiAssets/global/config/remote work | iwazaru + chipmunk | GET / must return real shipped assets, plus full global route strict replay and shutdown checks |

## Ownership census

All captured HTTP rows have an accountable current domain owner and API/bat review.
The non-HTTP rows labelled next-wave are explicitly not assigned to a current implementation
worker. They were reported to root, which acknowledged them and will reassign workers after
the immediate wave. They remain incomplete until those assignments and public checks land.
No recursive workers were created by parity.

## Current validation record

- `xtask --test parity`: four tests passed. Parser defaults, exact categories, domain mapping,
  orphaned ledger and duplicate fixture IDs were exercised through the real tooling API.
- First strict Process build was blocked by the shared lockfile while root added pinned
  dev dependencies. No lockfile changes were made by parity.
- Next strict Process build reached the actual runtime crate, then failed on the in-flight
  `legacy/locations.rs` import of `legacy::revert::LegacyRevert`. Session owner notified.
- Complete owner-partition tests, all-target strict lint, composed replay measurements and
  scoped commit IDs will be appended after execution. These lines do not claim success.
