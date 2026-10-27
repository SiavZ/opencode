# Phase 12 console baseline inventory

Latest-source local implementation and controlled acceptance are complete per coordinator `336372w6kq`, exit 0 at 2026-10-27 05:57:07.180901 UTC. Fmt and strict all-target clippy `-D warnings` passed. Ordinary 30 targets: **208 passed, 0 failed, 2 ignored** = hosted 84 + CLI 2 + adapters 88 + domain 34. Three same-chain explicit rebuilt browsers each passed 1/0/0: production console, development console and actual native support built-SSR Chromium; support canary scan passed. Do not double-count these as an ordinary total of 211. Final Bun at 05:52:59 UTC: console 11/0/275 assertions, support 3/0/39 assertions. Support UI is local/operator-only, not public-authenticated. Public artifact parity and native h2c behavior are proven locally; production backend remains absent (`deployable:false`), TLS/ALPN/proxy/upgraded cancellation unproven, and no Phase01 modularity approval exists. Earlier failed or pending snapshots below are superseded by this final receipt; no deployment or commit occurred.

## Scope and evidence rules

`phase-90/execution-map.md:20` assigns `packages/console/*` and console infrastructure to phase 12. Phase 13 owns sharing, analytics and sites for the other packages at line 21. An external upstream in the TypeScript baseline remains an external upstream in the Rust port; replacing the docs, stats, GitHub or enterprise service itself is not implied by porting its proxy. Retaining the entire TypeScript console implementation behind a proxy does not establish native console parity.

`docs/plans/rustport/coverage.md:1-18` describes core OpenAPI/CLI registry enumeration and explicitly denies release signoff. It does not enumerate console Solid actions or these auxiliary routes. `contract.md:28-30` requires isolated fixtures and explicit unsupported behavior. Existing helper tests are not public-route acceptance.

Scope scan found 42 machine HTTP route files and 18 operator scripts. Final `336372w6kq` ordinary 208/0/2 plus three explicit browser passes supersedes historical 201/0/1 (`188037lz69`), 203/0/1 (`460019s48h`) and the repaired `822402cbat`/`0970200rpt` failures. Provider-format evidence includes 529 captured TypeScript fixtures. Native-plan `487356un59` diff 6/guard 3 and `526024axhp` cloud 15/golden checks passed separately; do not add these counts to the ordinary aggregate.

Paths below are relative to `packages/console/app/src/routes/` unless fully qualified. M6 means GET, POST, PUT, DELETE, OPTIONS and PATCH, exactly the exports in the baseline. Axum GET also handles HEAD; HEAD is not a separately exported baseline operation.

## Machine HTTP routes

| Baseline path and methods | Source evidence | Current disposition and controlled proof |
| --- | --- | --- |
| GET `/auth` | `auth/index.ts:7` | Native session/Black-membership and migration redirects implemented. Auth/issuer controlled acceptance passed in final `460019s48h`; existing grant/cookie migration remains separate. |
| GET `/auth/authorize` | `auth/authorize.ts:4` | Native client redirects to configured issuer with Redis state bound to the sealed session. `fixture_oauth_authorize_callback_signed_jwt_redis_binding_and_replay` passed in final `460019s48h`; dedicated native issuer is a separate service, not this client route. |
| GET `/auth/{callback...}` | `auth/[...callback].ts:8` | Native `/auth/callback` and suffix handlers implemented; local issuer + Redis signed-JWT binding/replay fixture passed in final `460019s48h`. This proves fresh controlled login, not import of existing Cloudflare grants/cookies. |
| GET `/auth/status` | `auth/status.ts:4` | Rust session read/seal route exists. |
| GET `/auth/logout` | `auth/logout.ts:5` | Rust account-selection logout route exists. |
| POST `/stripe/webhook` | `stripe/webhook.ts:14-393` | Native signed webhook handles checkout/customer metadata, Lite/Black lifecycle, invoice success/failure/action-required and refunds. `fixture_signed_billing_lifecycle_and_action_operations` and adapter lifecycle fixtures passed in final `460019s48h`; local Stripe captures, not production payments. |
| POST `/api/support/actions/block-workspaces` | matching source `:9-21` | Native guarded batch mutation implemented. `support_authentication_precedes_validation_and_sql` and `fixture_support_block_batches_and_account_erasure_persist` passed in final `460019s48h`, covering authorization and persisted batch effects. |
| POST `/api/support/actions/unblock-workspaces` | matching source `:9-21` | Native guarded batch unblock implemented; authorization/validation and durable batch fixture coverage passed in final `460019s48h` (`support_authentication_precedes_validation_and_sql`, `fixture_support_block_batches_and_account_erasure_persist`). |
| POST `/api/support/actions/unblock-workspace` | matching source `:9-20` | Native guarded single-workspace unblock implemented; controlled support route/batch fixtures passed in final `460019s48h`. No per-flow browser execution is inferred. |
| DELETE `/api/support/actions/delete-account` | matching source `:9-20` | Native guarded transactional account erasure implemented; `fixture_support_block_batches_and_account_erasure_persist` passed in final `460019s48h`, asserting durable erasure effects. |
| POST `/api/support/actions/reset-quota` | matching source `:12-23` | Native guarded persistent quota reset implemented; actual HTTP `fixture_support_reset_referral_and_lookup_routes` and adapter `fixture_support_quota_referral_and_lookup_are_durable` passed in final `460019s48h`, not helper-only proof. |
| POST `/api/support/actions/create-referral` | matching source `:12-24` | Native guarded referral route uses the shared lifecycle; `fixture_support_reset_referral_and_lookup_routes` and durable support/referral adapter fixtures passed in final `460019s48h`. No independent browser-flow claim. |
| POST `/api/enterprise` | `api/enterprise.ts:61-113` | Native honeypot/localized validation and Salesforce/SES/EmailOctopus result policy implemented. Enterprise honeypot/locale/delivery-failure and one-channel-success fixtures passed in final `460019s48h`; loopback captures only, no live delivery. |
| POST `/honeycomb/webhook` | `honeycomb/webhook.ts:40-104` | Native guarded alert handler implemented. `support_honeycomb_capture_formats_incidents_and_ignores_resolved`, `support_honeycomb_reports_discord_rejection` and authentication fixtures passed in final `460019s48h`; controlled Discord capture, not live notification. |
| GET `/oauth/opencode/client.json` | `oauth/opencode/client.json.ts:17-37` | Native JSON metadata uses trusted public origin, cache/CORS and loopback redirects. `fixture_public_metadata_redirects_and_localized_validation` passed in final `460019s48h` after response-wrapper fixes; unrelated to hosted issuer login. |
| GET `/openapi.json` | `openapi.json.ts:1-7` | Native fixed-upstream fetch and JSON decode implemented; public metadata/proxy controlled fixtures passed in final `460019s48h` after response-wrapper fixes. Upstream remains external by baseline; no live fetch or per-flow browser proof is claimed. |
| GET `/discord` | `discord.ts:3` | Auxiliary public owner. 302 fixed Discord invite. |
| GET `/feishu` | `feishu.ts:3-7` | Auxiliary public owner. 302 fixed Feishu link, preserving query. |
| GET `/desktop-feedback` | `desktop-feedback.ts:3` | Auxiliary public owner. 302 fixed Discord feedback invite. |
| GET `/download/{channel}/{platform}` | `download/[channel]/[platform].ts:22-55` | Auxiliary public owner. Six desktop assets, stable/latest vs all-other/beta, upstream failure passthrough, missing asset 502, unknown platform 404. External update service remains external. |
| M6 `/stats` and `/stats/{...path}` | `stats/index.ts:3-8`, `stats/[...path].ts:3-8`, `lib/stats-proxy.ts:74-81` | Auxiliary public owner. 308 to same public origin `/data` suffix preserving query. No analytics implementation needed for redirect. |
| M6 `/docs` and `/docs/{...path}` | `docs/index.ts:5-23`, `docs/[...path].ts:5-23` | Auxiliary public owner. Stage-specific external docs origin, locale path/tag/cookie, original method/query/body and response semantics. Native docs site remains phase 13. |
| M6 `/s/{id}` | `s/[id].ts:5-23` | Auxiliary public owner. Same docs upstream with `/docs/s/{id}` localization and locale cookie. |
| M6 `/t/{...path}` | `t/[...path].tsx:4-19` | Auxiliary public owner. External enterprise proxy preserves baseline double slash before request pathname, optional explicit/cookie locale only. Native enterprise renderer is a different service. |
| M6 `/data` and `/data/{...path}` | `data/index.ts:3-8`, `data/[...path].ts:3-8`, `lib/stats-proxy.ts:11-184` | Native `public/data.rs` now implements locale redirect/cache keys, HTML asset rewrites, manual redirect rewriting, bypass assets and cache headers. Bounded process-local cache is explicit runtime adaptation. Local fixtures passed in full hosted task `9096886vms`, including cache/locale/Markdown/manual redirects, expiry/upstream failure and real gzip decode. |
| GET/OPTIONS `/changelog.json` | `changelog.json.ts:12-30`, `lib/changelog.ts:39-151` | Native `public/changelog.rs` now ports release sections and grouped image/video highlights, 503 fallback, CORS, success/error response TTLs and five-minute local cache. Local parser/cache/CORS/failure fixtures passed in task `9096886vms`. |
| POST `/bench/submission` | `bench/submission.ts:14-35`, core `schema/benchmark.sql.ts` | Auxiliary public owner. Localized required-field validation, generated benchmark ID and MySQL insert. No baseline authentication requirement. |

## Zen API routes and shared runtime

| Baseline path | Methods / evidence | Disposition |
| --- | --- | --- |
| `/zen/v1/chat/completions` | POST/OPTIONS, `zen/v1/chat/completions.ts` | Gateway/formats owners. |
| `/zen/v1/messages` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/v1/responses` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/v1/systemone` | POST/OPTIONS, matching source | Gateway/formats owners; do not omit specialized upstream format. |
| `/zen/v1/models/{model}` | POST/OPTIONS, `zen/v1/models/[model].ts` | Gateway/formats owners; preserve action suffix encoding. |
| `/zen/v1/models` | GET/OPTIONS, `zen/v1/models.ts` | Full Zen catalog with API-key workspace-disabled filtering is baseline; initial implementation had catalog assignment inverted. Gateway owner correcting and fixture comparison required. |
| `/zen/go/v1/chat/completions` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/go/v1/messages` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/go/v1/responses` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/go/v1/systemone` | POST/OPTIONS, matching source | Gateway/formats owners. |
| `/zen/go/v1/models` | GET/OPTIONS, matching source | Lite/Go catalog is baseline, unfiltered by key. Initial implementation had catalog assignment inverted; gateway owner correcting. Mutation controls are tenant owner. |
| `/zen/go/v1/usage` | GET/OPTIONS, matching source | Gateway owner; wire quota windows and API-key authentication. |

`zen/util/` files are implementation modules, not independent URL endpoints. Required modules are `handler`, `requestBody`, `modelsHandler`, `variant`, `pricing`, `trainingConsent`, `usageBatcher`, `logger`, `error`; limit/state modules are `ipRateLimiter`, `keyRateLimiter`, `modelTpmLimiter`, `modelTpsLimiter`, `providerBudgetTracker`, `stickyProviderTracker`, `trialLimiter`, `redis`; provider modules are `anthropic`, `google`, `openai`, `openai-compatible`, `systemone`, `provider`. All are gateway/formats obligations. `limits.rs` was a comment-only stub at audit start. `redis.ts:20-28` also defines checkout caps used outside inference. Admission without accounting, cancellation/stream settlement, routing budgets and reset windows does not establish full gateway parity.

## Browser pages, layouts and Solid actions

All page rendering remains SolidStart unless the browser owner proves an explicit alternative. The Rust JSON bridge is not SolidStart RPC wire compatibility. Every row below needs actual generated browser action/query requests mapped to Rust and checked for DB effects and admission. Pure display pages may remain compiled Solid frontend artifacts.

| Page / source group | Server behavior / disposition |
| --- | --- |
| `/`, `index.tsx` | Landing frontend. Keep compiled assets and middleware behavior; not a missing native DB route. |
| `/zen`, `zen/index.tsx` | Zen marketing frontend plus auth navigation. |
| `/go`, `go/index.tsx` | Go marketing frontend, referral query attribution and locale middleware. |
| `/black`, `black.tsx`, `black/index.tsx`, `black/common.tsx`, `/black/workspace` | Black enrollment/workspace selection actions need billing/browser ownership; not merely static marketing. |
| `/workspace`, `workspace.tsx`, `workspace-picker.tsx`, `user-menu.tsx` | Auth layout, account/workspace picker, logout navigation. Existing REST list/create is only backend portion. |
| `/workspace/{id}`, `[id].tsx`, `[id]/index.tsx`, `new-user-section.tsx` | Workspace admission, default region, usage existence, keys/models/providers. Tenant/browser owners. |
| `/workspace/{id}/settings`, settings `index.tsx`, `settings-section.tsx:12-49` | Settings query/update. Existing Rust workspace info/update helpers and routes; browser binding required. |
| `/workspace/{id}/members`, members `index.tsx`, `member-section.tsx`, `role-dropdown.tsx` | List/invite/update/remove, self-demotion/removal guards, monthly-limit input semantics. Rust routes exist; invitation signup/claim is separate issuer gap. |
| `/workspace/{id}/keys`, keys `index.tsx`, `key-section.tsx` | List/create/remove, own-key secret visibility and admin cross-owner rules. Existing REST routes need complete HTTP lifecycle fixtures. |
| `model-section.tsx:43-100` | `model.info`, `model.toggle`. Tenant owner wires existing disabled_models/enable_model/disable_model helpers. |
| `provider-section.tsx:22-56` | `provider.list`, `provider.save`, `provider.remove`. Tenant owner wires existing BYOK helpers. Baseline returns raw credentials and masks only rendering; do not describe that as server-side secret redaction. |
| `/workspace/{id}/billing`, billing `index.tsx`, `billing-section.tsx`, `payment-section.tsx` | `billing.get`, portal, payments, receipt. Billing/browser owners. |
| billing `reload-section.tsx:13-57`, `monthly-limit-section.tsx`, `redeem-section.tsx` | Manual reload, reload settings via direct SQL, monthly limit, email coupon redemption. Billing/browser owners. |
| billing `black-section.tsx:18-109` | Subscription usage windows, waitlist cancel, enroll, Black useBalance. Direct UI SQL means core Billing exports alone omit operations. |
| `/workspace/{id}/go`, Go `index.tsx`, `lite-section.tsx:34-310` | Lite status/usage detail windows, checkout/payment methods/coupons, portal, Lite useBalance, provider region routing, training consent. Billing + tenant + gateway + browser ownership must agree. |
| `/workspace/{id}/usage`, usage `index.tsx`, `usage-section.tsx` | Paginated usage list, billing/browser owners. |
| usage `graph-section.tsx:27-101` | Separate monthly timezone-aware model/key/plan cost aggregation and historical deleted-key labels. Native tenant endpoint now implemented; projection and overflow-month fixtures passed in full hosted task `9096886vms`. It remains distinct from Billing.usages. |
| `workspace/common.tsx:42-125` | Last workspace, session flags, checkout rate cap, billing projection. Browser/billing/tenant owner. |
| `component/go-referral.tsx:27-46`, `lib/referral-invite.ts:32-43` | Referral summary/preview/reward apply, cookie capture/clear. Referral owner required; pure normalize helper insufficient. |
| `/bench`, `bench/index.tsx`, `/bench/{id}`, `bench/[id].tsx` | Benchmark DB list/detail, frontend rendering and localized content. Native GET `/benchmarks` and `/benchmarks/{id}/tasks/{task}` implement list/detail projections alongside submission. Same MySQL fixture passed submit/list/task/null lifecycle in task `9096886vms`; generated browser binding mapped, with coordinator-reported production/development workflow passes. Per-operation benchmark browser assertions are not inferred solely from that aggregate browser receipt. |
| `/changelog`, `changelog/index.tsx` | Frontend plus shared changelog fetch/parser. Native `/changelog.json` implemented; generated browser query must unwrap `.releases` and retain empty array for upstream 503. |
| `/download`, `download/index.tsx`, `download/types.ts` | Display/frontend platform selection; machine redirect separately inventoried. `types.ts` is not a URL. |
| `/enterprise`, `enterprise/index.tsx` | Display frontend and enterprise form POST described above. |
| `lib/github.ts:4-39` | Native GET `/github.json` concurrently fetches fixed repository meta/releases/contributors, projects first release and Link count, returns null for undefined baseline result. Browser owner mapped endpoint; local metadata/null/Link fixture passed in task `9096886vms`. |
| `/brand`, `brand/index.tsx` | Static frontend and downloadable brand assets. |
| `/legal/privacy-policy`, `/legal/terms-of-service` | Static localized legal frontend. |
| `/temp`, `temp.tsx` | Retained baseline page; do not erase because non-production-looking. |
| `[...404].tsx` | Frontend not-found handler. |

## Authentication service and invitation lifecycle

`packages/console/function/src/auth.ts:42-116` is an OpenAuth issuer Worker with GitHub OAuth, Google OIDC, KV-backed token/state, subject schema and allowlisted authorization redirects. Lines 123-204 verify provider identity/email/stage, choose linked account, claim email invitations, require Black membership and issue account subject. At initial audit, `auth/issuer.rs` contained only success semantics and the client callback did not implement the issuer. Current native service `console/issuer.rs` and durable adapter `auth/issuer_runtime` implement that separate runtime; public issuer/installed SDK and OAuth/Redis fixtures passed in historical `460019s48h`. Client and issuer remain separate services.

Required acceptance is invite POST -> captured mail link -> provider login fixture -> durable invitation account binding + default API key -> admitted workspace, with denied/unverified identity and account/invite replay cases. Initial invitation/store helpers alone did not prove this chain. Current public native issuer fixtures cover provider admission and MySQL invitation claim with the installed OpenAuth SDK, passing in historical `460019s48h`; existing grant/cookie import remains absent.

`auth-redirect.ts` and its test define issuer allow rules, not another Worker. Mail template `mail/emails/templates/InviteEmail.tsx` and SES adapter are internal dependencies; current Rust mail implements delivery through fixtureable SES and swallows mail failures like baseline.

Issuer worker update: dedicated owner implemented fresh native issuance using durable SQLite rather than Cloudflare KV. Existing KV codes/refresh grants and encrypted browser cookies are not imported, requiring reauthentication or explicit migration. Retained ES256/RS512 public keys are configurable; legacy encrypted cookies and existing KV codes/refresh grants are not imported. See `bins/hosted/src/console/issuer/DEPLOYMENT.md`; new implementation is not blanket rolling-cutover compatibility.

## Workers and infrastructure

| Baseline | Behavior | Disposition |
| --- | --- | --- |
| `packages/console/function/src/auth.ts` | OpenAuth issuer runtime | Native independent issuer implemented with durable SQLite and signing/JWKS; public issuer 2 and adapter 7 plus installed SDK acceptance passed in historical `460019s48h`. Existing Cloudflare grant/cookie import is not implemented. |
| `packages/console/function/src/stat.ts:6-42` | POST IDs -> last 30 one-minute TPS qualify/unqualify buckets, non-POST 405 | Native console-stat service implemented; hosted stat fixture and adapter real-MySQL 30-bucket aggregation passed in historical `460019s48h`. Native topology descriptors replace the initial TS plan outputs; actual provisioning remains absent. |
| `packages/console/function/src/log-processor.ts:5-94` | Tail inference POSTs, merge `_metric` records, request geo/status/duration and IPv4 /32 or IPv6 /64 enrichment | Native authenticated tail ingress and generation -> TailEmitter -> Capture::Http implemented; HTTP ingress/capture and actual generation fixtures passed in historical `460019s48h`. Native descriptors replace initial TS plan outputs. Baseline Honeycomb export is commented out, not a required live behavior; Cloudflare tail attachment remains separate. |
| `packages/console/support/src/routes/{index,lookup}.tsx`, `support/src/lib/lookup.ts:41-328` | Operator lookup by account/email/workspace/key, billing/usage/member/payment breakdown | Native guarded support lookup API implemented; actual HTTP reset/referral/lookup and durable adapter fixtures passed in historical `460019s48h`. Actual native support built-SSR Chromium/MySQL and canary scan passed in final `336372w6kq`. Frontend scope is isolated local/operator-only; no public authentication claim. |
| `packages/console/resource/{resource.cloudflare,resource.node}.ts` | Runtime Resource bindings | Rust config reads env, but an explicit secret/catalog/origin/service binding handoff is required. |
| `infra/console.ts`, Rust `xtask/src/cloud/infra/console.rs:143-146,229-243,419-427` | Auth worker, tail worker, stat worker, SolidStart frontend, linked secrets/prices/catalog | Initial audit found TypeScript handler paths. Infra owner now reports `rust/xtask/src/cloud/infra/console/native.rs` replaces default AuthApi/AuthStorage/Console/Stat/LogProcessor outputs with Linkable native descriptors marked `deployment_backend_required`, `deployable:false`, configuring durable issuer mounts, Redis, model/env fragments, co-located Node SSR and tail ingress/producer. Coordinator reports native plan `487356un59` diff 6/guard 3 and `526024axhp` cloud 15 tests/golden check green; these prove plan validation, not deployment. No container/TLS/volume/secret provisioning backend exists; Cloudflare export attachment is separate. No production deployment is authorized by this audit. |

## Operator scripts

These are CLI/operator operations, not anonymous web routes. At initial audit no native entrypoint existed. Worker owner now reports all 18 stems executable through `OPENCODE_HOSTED_SERVICE=console-worker OPENCODE_CONSOLE_WORKER_COMMAND=<stem> opencode-hosted`, JSON stdin, with `workers::run_from_env` mounted before main ConsoleConfig. The table preserves initial missing-scope evidence; current source mapping is this exact shared entrypoint for each filename stem. Worker owner verified task `188037lz69` exit 0 at 04:47:21 UTC, including console_workers 4, config 1 covering all five secret commands, operators 1 covering actual loopback Stripe gift/transfer and full CSV/cutoff, and stats unit 1. These close the recorded local fixture gate; they do not prove production payments or deployed SST secret promotion. DB operations require a loopback `p12_` database and exact `OPENCODE_CONSOLE_OPERATOR_CONFIRM_DATABASE` plus adapter confirmation. Gift/transfer accept only literal-loopback `stripeCaptureUrl`. No live operator side effects are authorized.

| `packages/console/core/script/` file | Required behavior / current disposition |
| --- | --- |
| `black-cancel-waitlist.ts:4-39` | Validate workspace/billing, clear booking/selection. Native same-stem JSON-stdin command now implemented; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `black-onboard-waitlist.ts:4-38` | Validate booking and set selected time. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `black-select-workspaces.ts:6-41` | Select eligible waitlisted plan workspaces and print unique emails. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `black-gift.ts:9-115` | Create gifted Stripe subscription/customer and transactional membership plan rows. Native same-stem command uses literal-loopback Stripe capture only; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `black-transfer.ts:5-163` | Move subscription/balance/payment state and Stripe customer metadata transactionally. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `black-stats.ts:8-312` | Per-plan historical usage/spend/model/token reports. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `create-api-key.ts:12-146` | Parse flags, create/reuse account/workspace/user/billing and key. Native same-stem orchestration command now creates/reuses complete account/workspace/user/billing/key; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `create-coupon.ts:4-24` | Validate email/type and insert coupon. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `credit-workspace.ts:6-35` | Validate workspace and grant dollar credit. Native same-stem command implemented; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `disable-reload.ts:5-34` | Validate workspace/billing and disable reload. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `freeze-workspace.ts:8-39` | Revoke available balance through signed negative grant. Native same-stem JSON-stdin command implemented per worker owner; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `lookup-user.ts:20-386` | Identifier dispatch, verbose DB diagnostic reports. Native same-stem command implemented, separate from support web lookup; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `reset-db.ts:11-13` | Destructive selected-table reset. Native same-stem command requires exact loopback p12 database confirmation; isolated reset/rollback fixture passed in `188037lz69`; no destructive real data execution authorized. |
| `promote-limits.ts:7-22` | Read fallback limit secret and promote stage. Native local config-store stage promotion implemented; does not write deployed SST secrets; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `promote-models.ts:8-33` | Read 30 model fragments and load destination stage. Native local config-store stage copy implemented, not deployed secret promotion; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `pull-models.ts:8-33` | Read 20 baseline fragments and load fallback. Preserve actual baseline count or record approved fix, do not assume 30. Local config-store acceptance passed with all five secret commands in `188037lz69`; no deployed secret promotion claimed. |
| `update-limits.ts:8-30` | Export/edit/validate/reload limits. Native edited-JSON local store workflow replaces vim; no cloud mutation; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `update-models.ts:8-43` | Export/edit/validate/split 30 catalog fragments and reload. Native edited-JSON/split local store workflow implemented; no cloud mutation; local fixture matrix passed in task `188037lz69`; no production execution claimed. |
| `packages/console/app/script/generate-sitemap.ts` | Native sitemap/config/tui artifact parity now demonstrated: owner normal-front-proxy HTTP 200 exact-byte check at 05:35:22 UTC (97,390 / 39,039 / 1,156,030 bytes), plus coordinator Bun 11/0/275 at 05:39:13 UTC including 3 artifact parity tests. Final `336372w6kq` passed fmt/strict all-target lint, full-package ordinary 208/0/2 and all three explicit browsers at 1/0/0 each, superseding those earlier failures. Final Bun console 11/0/275 and support 3/0/39 at 05:52:59 UTC passed separately. |

### Native worker update

Worker owner reports `console-stat` standalone HTTP and `console-log-processor` or `console-log` mode POST `/tail`, bearer `OPENCODE_CONSOLE_LOG_INGRESS_TOKEN`, required capture FILE/URL sink, authorization before 8 MiB parse, response accepted only after capture. Native trace emission into that ingress is a separate integration obligation for gateway/infra. Actual baseline stat/log/Black CSV capture comparison passed. Worker owner subsequently verified task `188037lz69` exit 0 at 04:47:21 UTC: adapter console_workers 4, config 1, operators 1 and stats unit 1 passed; current hosted tail 2/stat 1 and CLI 2 passed. Actual generation -> TailEmitter -> authenticated tail_router -> Capture::Http proof is inside `console::zen::acceptance::fixture_native_sse_records_usage_after_eof_and_disconnect_drops_provider`, which passed in both `9096886vms` and `188037lz69`. Gateway owner identifies `bins/hosted/src/console/zen/acceptance.rs:238-370` as the proof, asserting usage/tokens total 700, timing/status and secret redaction, without a fake Completion or mock emitter. Component/endpoint assertions added at 04:46:05 UTC passed in the final settled-source hosted rerun `610237ff8k`, per coordinator. No remaining generation-to-tail proof blocker is recorded.

The five secret commands `promote-limits`, `promote-models`, `pull-models`, `update-limits`, `update-models` execute explicit native local config-store atomic/fsync stage operations. They do not execute SST or update deployed cloud secrets. Edited JSON replaces interactive vim. This is an explicit offline operator interface adaptation, not production secret promotion parity. Black stats returns full CSV with baseline January 31 cutoff. See `bins/hosted/src/console/workers/README.md`.

## Current acceptance and boundaries

Final coordinator `336372w6kq` closes local controlled acceptance: fmt, strict all-target lint, ordinary 208/0/2 across 30 targets and three separate explicit browsers at 1/0/0 each. This supersedes `822402cbat` lookup/countdown failure and `0970200rpt` selector failure; deterministic same-clock `lookup_at` and boundary regression fixes passed. Historical `460019s48h` is not the latest aggregate.

- Public sitemap/config/tui normal-front-proxy HTTP 200 exact-byte proof: 97,390 / 39,039 / 1,156,030 bytes at 05:35:22 UTC; artifact parity tests passed.
- Transport `1823856h1y` 6/0 and gateway `539306b5qv` 5/0 establish h2c same-connection reset, sibling/reuse, URI regressions and reset/no-settlement/no-completion-tail followed by EOF/full-tail; final hosted suite retains this coverage. TLS/ALPN, proxies and upgraded cancellation remain uncertified.
- Actual native support built-SSR Chromium/MySQL and canary scan passed in the final chain. Support UI is isolated local/operator-only; public authentication or deployment is not claimed.
- Native production provisioning remains absent (`deployment_backend_required`, `deployable:false`). No deployment is authorized by these receipts. Offline local secret-store and process-local cache adaptations remain explicit.
- Fresh issuer acceptance passed; deliberate reauthentication is a supported cutover choice. Seamless grant/cookie import is not mandatory absent an explicit request and required keys; existing records are not imported.
- No Phase01 modularity approval exists. No code, Cargo runs or commits were made during this documentation update.

Coordinator reports final cleanup at 2026-10-27 05:58:31 UTC: session Redis `p12-eni-final-20261027-0541` verified absent; preexisting MySQL remained up and untouched. Git diff check passed and packages status clean. No deployment or commit.