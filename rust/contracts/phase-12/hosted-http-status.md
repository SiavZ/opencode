# Hosted console HTTP status

As of 2026-10-27. Latest-source local implementation and controlled acceptance are complete per coordinator `336372w6kq`, exit 0 at 2026-10-27 05:57:07.180901 UTC. Fmt and strict all-target lint passed; ordinary 30 test targets passed **208/0/2**, with three separate rebuilt browser runs each **1/0/0**. Support frontend scope is isolated local/operator-only. Native production provisioner remains absent; TLS/ALPN/proxy/upgraded cancellation unproven; no deployment or Phase01 modularity approval is claimed.

## Runtime boundaries

- `opencode-hosted` serves native Axum console APIs. `console/mod.rs` composes auth, workspace/settings, billing, browser delegation, referrals, public routes, Zen and support.
- The unchanged SolidStart frontend is compiled with `rust/scripts/console-browser/plugin.ts`. Actual SolidStart owns generated server-function IDs, Seroval framing, FormData, SSR, redirects and revalidation. The plugin delegates server bodies to authenticated Rust APIs; it does not emulate `/_server` JSON wire compatibility.
- `build.ts` produces a Node/Nitro production bundle outside `packages/`. `production-start.mjs` routes native endpoints to the colocated loopback Rust service and frontend requests to compiled SSR. A Cloudflare Worker cannot use localhost to reach an unrelated native deployment.
- `OPENCODE_HOSTED_SERVICE=console-issuer` exposes the native issuer on its own listener/origin. Console mode does not expose issuer endpoints.
- `console-stat`, `console-worker`, and `console-log-processor` select native TPS aggregation, bounded JSON stdin commands, and authenticated HTTP tail ingress. Workers start without unrelated console authentication configuration; log ingress does not require a database.
- `transport.rs` attaches connection-scoped cancellation to requests. Real HTTP/1 TCP disconnects before provider headers and during streaming cancel owned work. Normal Content-Length/chunked request-body completion does not cancel reusable connections.

## Implemented HTTP and browser behavior

The complete baseline inventory, source references and route dispositions are in `inventory.md`.

### Authentication and invitation admission

`/auth/authorize`, `/auth/callback` and suffixes, `/auth/status`, `/auth/logout`, `/auth`, and `/auth/action/actor` preserve session/account/workspace behavior. Authorize persists a ten-minute one-time nonce in Redis, bound to the sealed session. Callback atomically consumes state and verifies ES256/RS512 issuer JWKS, issuer, audience, expiration, access mode and account subject before sealing the session. Query errors omit secret code/state values. Actor mutations enforce the shared browser-origin boundary.

The native issuer implements GitHub OAuth and Google OIDC, provider state/nonce validation, allowed redirects, PKCE, single-use authorization codes, signing/JWKS/discovery, durable refresh rotation, and the existing account/invitation/Black-membership admission flow. SQLite transactions provide native persistent single-writer grant storage; signing keys and durable paths must be configured explicitly. Public tests exercise the installed TypeScript OpenAuth SDK against the native service.

Invitation POST requires an admitted workspace administrator and same-origin browser request. SES delivery is fixtureable; invitation claiming is exercised through native issuer admission, not inferred from a repository helper alone.

### Workspace and settings

Native routes cover workspace list/create/read/update/delete/default-region, members, keys, BYOK provider list/save/remove, model listing/toggle and monthly usage graphs. Graph queries preserve timezone offsets, model/key/plan grouping, historical deleted-key labels and workspace isolation. MySQL strict `ONLY_FULL_GROUP_BY` behavior is tested. Existing baseline authorization conventions, including member workspace deletion, remain explicit rather than silently replaced with generic admin assumptions.

### Billing and referrals

Stripe raw-body signature validation precedes processing. Native handling covers checkout credits, first-customer metadata, customer updates, Lite/Black subscription lifecycle, invoice success/failure/action-required paths, refunds and replay handling. Authenticated browser operations cover summaries, payment/usage lists, receipt/portal/checkout invocation, reload settings, monthly limits, coupons and subscription settings according to the active baseline behavior.

Referral persistence covers attribution, summaries, preview, reward application and Lite-subscription completion. Reward application updates all intended usage counters atomically and does not become a balance credit. Tests cover tenant isolation, duplicate application, account history/deleted inviter eligibility, empty administrator-email fallback and concurrent completion in two different workspaces. Referral normalization uppercases before stripping/truncation, including Unicode expansions.

### Zen, Go and provider transport

- `GET /zen/v1/models` uses the full `zenModels` catalog and live-key workspace-disabled filtering.
- `GET /zen/go/v1/models` uses the public Lite/Go catalog. The earlier inverted assignment was corrected against the actual TypeScript routes.
- Native generation routes cover chat completions, Anthropic messages, Responses, Systemone and the baseline Google action path, with OPTIONS/CORS and configured migration forwarding where the baseline delegates.
- Provider helpers, usage parsing, request/response conversions, incremental SSE/UTF-8 handling and Bedrock framing are tested against captured executions of the actual TypeScript converters. Unsupported baseline format combinations remain typed failures, not invented converters.
- IP/key/checkout caps, trials, TPM/TPS, provider budgets/sticky routing, plan/credit/BYOK admission, rolling windows, reload locking and durable settlement are implemented and fixture-tested.
- Hot-workspace Redis accumulation follows baseline pending-delta/GETDEL behavior. It is not represented as a stronger crash-safe queue than the baseline.
- Completed SSE settles usage once at EOF; downstream cancellation does not settle a completed generation. Provider/internal secret metadata is not leaked.
- Native gateway metrics flow through the authenticated tail ingress into a durable capture. Finalized usage/timing and secret-free event assertions are exercised through real generation requests.

### Support, public routes and operators

Native support routes include guarded mutations and lookup, enterprise intake with local-capture Salesforce/SES/EmailOctopus adapters, Honeycomb-to-Discord formatting and newsletter signup. All provider calls used for acceptance are loopback captures.

Auxiliary routes cover native-client metadata, fixed redirects, OpenAPI, update-download redirects, locale-aware docs/share/enterprise proxies, stats-to-data redirects, the data proxy/cache/HTML transformations, GitHub metadata, changelog parsing/cache/CORS and benchmark submission/list/task reads. Compressed upstream HTML is decoded before rewriting and stale encoding/length headers are not retained. Configured upstream destinations ignore attacker-controlled Host headers.

All eighteen inventoried operator script equivalents have safe native local invocation paths. Destructive operations require an explicitly confirmed disposable local database. Stripe operator acceptance uses literal-loopback capture. Secret pull/update/promote equivalents use a marked durable local configuration store; they are not a claim of live SST secret-management support.

## Controlled verification

Coordinator reports final `336372w6kq` completed 2026-10-27 05:57:07.180901 UTC, exit 0. Fmt and strict all-target clippy `-D warnings` passed. First 30 ordinary result lines total **208 passed, 0 failed, 2 ignored** = hosted 84 + CLI 2 + adapters 88 + domain 34. All 10 public fixtures passed within the hosted suite.

Same-chain explicit acceptance rebuilt and passed production console, development console and actual Rust/MySQL native support built-SSR Chromium, each **1 passed, 0 failed, 0 ignored**. Support canary scan passed. The ordinary opt-in ignores are not silently counted as passes; 211 passes across every result line includes these three browsers and is not the ordinary suite total.

Console browser workflows exercise compiled SSR, generated Solid RPC, settings persistence/reload and controlled database effects; aggregate workflows do not prove every individual route was browser-tested. Support browser acceptance applies only to its isolated local/operator bridge/build, with no public authentication claim.

Final Bun suites at 2026-10-27 05:52:59 UTC: console **11 passed, 0 failed, 275 assertions**; support **3 passed, 0 failed, 39 assertions**. Public sitemap/config/tui normal-front-proxy HTTP 200 exact-byte checks at 05:35:22 UTC measured 97,390 / 39,039 / 1,156,030 bytes, with artifact parity tests passed.

Transport `1823856h1y` passed 6/0 and gateway `539306b5qv` passed 5/0; native h2c same-connection reset/sibling/reuse, URI regressions, no settlement/completion tail on reset and EOF follow-up/full-tail are retained in final hosted coverage. Native-plan diff/guard/cloud/golden receipts remain separate.

Final success supersedes `822402cbat` lookup/countdown and `0970200rpt` selector failures; deterministic same-clock lookup and boundary regression repairs passed. Historical `460019s48h` 203/0/1 is not the latest aggregate. Receipts are coordinator/owner-reported where not independently read. No Cargo or runtime edits were performed for this final documentation update.

## Deployment and compatibility limits

1. No live cloud deployment, payment, provider inference or email was performed. Production configuration, provider credentials, TLS ingress, secret mounts, durable storage and operational rollout remain environment work.
2. Native infrastructure plans explicitly identify `deployment_backend_required` and `deployable: false` where this repository lacks a native scheduler/volume/ingress renderer. A tested topology description and built SSR artifact are not provisioned production infrastructure. Existing Cloudflare tail sources need an explicit authenticated export attachment.
3. Existing Cloudflare KV authorization codes, refresh grants and encrypted browser cookies are not imported into SQLite. Deliberate user reauthentication is a supported cutover policy; seamless grant/cookie import is not mandatory absent an explicit request and required key material. Fresh issuer acceptance is complete per issuer audit. Retained ES256/RS512 public verification keys are supported; this does not migrate existing grants.
4. Coordinator reports `1823856h1y`: all 6 transport tests passed, including h2c pre-header and body reset with live same-connection sibling/reuse. Separate native gateway h2c acceptance `539306b5qv` passed 5/0, covering URI regressions, actual same-connection reset with no DB settlement/completion tail, and EOF follow-up with full tail. This supersedes the earlier failed metric assertion; it does not certify other deployments or protocols. TLS/ALPN, proxies and upgraded-protocol cancellation remain uncertified.
5. Native public caches are bounded process-local caches, not a global Cloudflare edge cache. Native issuer storage requires the documented durable single-writer deployment boundary.
6. No implementation commit was made in this shared dirty worktree. Unrelated existing edits and the TypeScript baseline remain preserved. Local controlled acceptance is not Phase01 modularity approval.

Coordinator reports final cleanup at 2026-10-27 05:58:31 UTC: session Redis `p12-eni-final-20261027-0541` verified absent. Preexisting MySQL remained up and untouched. Git diff check passed and packages status clean. Fixture cleanup is not deployment.
