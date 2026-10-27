# Console browser integration

This runs the existing console frontend source against Rust without editing `packages/` or deploying anything. It requires the installed Bun, Vite, SolidStart and Playwright dependencies. It includes local development and compiled Node SSR modes. Rust delegates data and actions; SolidStart renders the HTML.

## Why a JSON route alias cannot work

The installed `@solidjs/start` is `2.0.0-devinxi.0` from the pinned package snapshot. Its `src/server/server-runtime.ts` and `server-functions-handler.ts` define the actual protocol:

- The compiler generates opaque function IDs. The browser sends `X-Server-Id` and `X-Server-Instance` to `/_server`.
- Positional arguments use Seroval JSON nodes. A single FormData argument uses multipart. GET encodes Seroval in `args`; no-JavaScript forms use the generated `id` URL.
- Responses use executable Seroval chunks prefixed by `;0x` plus eight hexadecimal byte-count digits plus `;`. The content type is `text/javascript`.
- Router Responses carry `Location`, `X-Revalidate`, and sometimes `X-Single-Flight`. No-JavaScript submission uses redirects and flash cookies.
- Initial SSR calls the server functions directly. Replacing only `/_server` cannot replace the data source used during rendering.

`plugin.ts` rewrites the bodies of actual `"use server"` functions in memory before the installed SolidStart compiler processes them. It removes imports that became unused, including type-only imports whose loading would otherwise initialize SST resources. SolidStart still generates all IDs, serializes arguments/results, renders SSR, hydrates the browser, and processes forms and router responses. `runtime.ts` translates the original arguments to the allowlisted `/_console/browser` Rust endpoint. That endpoint invokes existing authenticated Rust route handlers, preserving their tenant checks and mutation authorization.

The middleware's pure referral normalizer is copied from its actual baseline AST during transformation, including the baseline `CODE_LENGTH`. Its account attribution is delegated to Rust. Baseline locale selection, response cookies, client source, JSX, CSS, static assets and translations stay unchanged.

## Local startup

Start Rust through the existing hosted binary with fixture-only configuration. At minimum the normal hosted process requires:

```sh
OPENCODE_CONSOLE_DATABASE_URL=mysql://root:p12fixture@127.0.0.1:33612/YOUR_ISOLATED_DATABASE
OPENCODE_CONSOLE_PUBLIC_ORIGIN=http://127.0.0.1:3001
OPENCODE_HOSTED_ADDR=127.0.0.1:8787
OPENCODE_CONSOLE_SESSION_SECRET=YOUR_LOCAL_SECRET_OF_AT_LEAST_32_BYTES
OPENCODE_CONSOLE_AUTH_STATE_REDIS_URL=redis://127.0.0.1:YOUR_LOCAL_REDIS_PORT
```

Authentication, migration destinations, model catalogs, quotas and integration credentials follow the normal module-owned configuration. Do not use real cloud credentials or production databases for this local launcher. Payment, email, model and GitHub integration tests must inject loopback fixture upstreams, not live services.

From the repository root:

```sh
OPENCODE_BROWSER_RUST_URL=http://127.0.0.1:8787 \
OPENCODE_CONSOLE_PUBLIC_ORIGIN=http://127.0.0.1:3001 \
OPENCODE_BROWSER_PORT=3001 \
bun rust/scripts/console-browser/start.ts
```

The frontend binds only `127.0.0.1` and requires its exact public origin. The Rust target must be a loopback HTTP origin without credentials, path or query. The launcher loads SolidStart's own Vite version, rather than the app's different Vite version, so the framework recognizes the runnable SSR environment. It stores caches and isolated HOME/XDG paths under `JCODE_SCRATCH_DIR` or `rust/target/console-browser`. It proxies `/auth` to Rust while browser pages and `/_server` remain in the real SolidStart runtime. Rust Set-Cookie and redirect headers propagate back through the current SSR/action response.

## Executable acceptance

Compiler inventory and source-preservation checks:

```sh
cd rust/scripts/console-browser
bun test acceptance.test.ts
```

Actual HTTP, browser, and database acceptance requires the local MySQL fixture on port 33612 and the already installed Playwright Chromium:

```sh
cd rust
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
export SCCACHE_BASEDIRS="$PWD/target" SCCACHE_CACHE_SIZE=100G
export CARGO_TARGET_DIR="$PWD/target/p12"
cargo test -p opencode-hosted console::browser -- --nocapture
cargo test -p opencode-hosted console::browser::tests::fixture_browser_ssr_and_action_round_trip -- --ignored --nocapture
```

The ignored fixture is opt-in because it needs MySQL and Chromium. Running only the first Cargo command is **not** browser acceptance. The second command creates and migrates a unique database, seeds an account, binds Axum and Vite on assigned loopback ports, seals a real auth cookie, runs Chromium, then closes both servers and drops the database. No production data or provider calls are needed.

The browser driver checks authenticated SSR, a real settings form submission through compiler-generated RPC, framed responses, revalidation, persistence after reload, provider FormData mutation, graph positional-argument Seroval query, billing limit mutation, and locale/referral middleware. The Rust fixture independently checks the workspace mutation in MySQL. Compiler operation coverage is a routing regression, not proof that every business flow renders or succeeds.

## Compiled SSR acceptance

Build the unchanged frontend through the native Node SSR configuration, then run the same real browser fixture against that built artifact:

```sh
# Repository root, local artifact generation only
bun rust/scripts/console-browser/build.ts

cd rust
OPENCODE_BROWSER_PRODUCTION_ENTRY="$PWD/target/console-browser-production/output/server/index.mjs" \
cargo test -p opencode-hosted console::browser::tests::fixture_browser_ssr_and_action_round_trip -- --ignored --nocapture
```

The fixture supplies unique front-proxy and SSR ports. `production-start.mjs` starts the actual Nitro Node server and proxies native APIs to Axum before reaching SolidStart. This topology requires both processes on the same host or shared loopback network. A Cloudflare Worker cannot use localhost to reach this Axum process. Build and deployment planning do not publish or provision anything.

Verified on October 27, 2026: compiler/runtime suite passed 6 tests with 243 assertions; Rust browser route tests passed 4 tests; explicit development Chromium fixture passed 1 test with 0 ignored; explicit compiled SSR Chromium fixture passed 1 test with 0 ignored. The production fixture ran the actual built Node bundle, not Vite dev. The broader development fixture additionally exercised provider, graph and billing action wire paths.

## What this does not establish

The production-mode fixture proves the locally built Node SSR bundle, authenticated rendering, real hydrated settings action, response framing, revalidation and persistence after reload. It does not establish a Cloudflare Worker deployment, CDN routing, real issuer login, real Stripe purchase, email delivery, external provider inference, or complete UI interaction matrix. The unchanged Go workspace route currently redirects to `/console/go` in the baseline; the bridge must not claim to restore that retired UI. The local frontend runtime and Rust process are both required. A production equivalent needs a separately reviewed frontend build/runtime and same-origin reverse-proxy deployment, with actual platform resource bindings and issuer callback URLs.