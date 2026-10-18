# Phase 90 provisional contract freeze (record v0)

Status: provisional, issued by the coordinator on 2026-10-03 to unblock parallel implementation. This is **not** the final Phase 90 approval described in `docs/plans/rustport/phase-90-architecture-contract-freeze.md`. Every decision below is binding for implementation work now, and reopenable through the change-control process at the bottom. Items marked *blocking proof* must be satisfied by the named phase before the decision becomes final.

Baseline: commit `907b3bc518fa48e90e8ec24dd327d13eee71c36c` (`dev`), plans at `7f9377147`. Work branch: `rust-port`.

## D1. Workspace and toolchain

- Workspace root `rust/`, members `crates/*`, `bins/*`, `xtask`. Toolchain pinned `1.99.0` (`rust/rust-toolchain.toml`), edition 2024, resolver 3.
- External crate versions are pinned in `rust/Cargo.toml` `[workspace.dependencies]`, retrieved from the crates.io index on 2026-10-03 as the newest non-prerelease, non-yanked version. *Blocking proof (01, 15)*: license, advisory (`cargo deny`/`cargo audit`), MSRV and target support recorded in `rust/contracts/phase-01/dependencies.md` before Phase 92.
- `unsafe_code = "deny"` workspace-wide. Exceptions need a coordinator-approved `#[allow]` with a documented invariant.

## D2. Crate graph

`rust/contracts/crate-edges.toml` is the exact first-party allowlist. Client, view-model and ui-web never depend on server, runtime, session-runtime, storage-sqlite or platform-native. *Blocking proof (01)*: `cargo xtask edges` enforces it on native and `wasm32-unknown-unknown` resolved graphs, with a deliberately broken fixture rejected.

## D3. Serialization authority

- JSON wire and storage values are byte-compatible with the TypeScript baseline, including key order (serde_json `preserve_order`), optional-key omission versus explicit `null`, number formatting and string escapes.
- `opencode-schema` owns identifiers and shared values. Phase 06 `opencode-protocol` owns wire DTOs, errors and event envelopes. Domain crates own service traits.
- Identifier encoding is the baseline's, including 48-bit time truncation (`schema/src/identifier.rs`, verified against bun 1.3.14 output).

## D4. Async and lifecycle

- tokio multi-thread runtime in bins, libraries are runtime-agnostic where practical but may require tokio. Cancellation via `tokio_util::sync::CancellationToken`. Owned task groups (`tokio::task::JoinSet` or `tokio_util::task::TaskTracker`) joined on shutdown.
- V2 session lifecycle contracts from `AGENTS.md` (repo root) and `architecture.md` "Runtime lifecycles" are frozen as written. *Blocking proof (04)*: historical message-only retry reconciliation discrepancy reproduced and resolved with a test before session-api is declared final.

## D5. Storage

- Local store: SQLite through `rusqlite` with the `bundled` engine, a bounded dedicated worker thread per connection pool, existing baseline PRAGMAs and the existing Drizzle migration journal applied in order. Rust reads and writes the same database file as the baseline (no schema fork). *Blocking proof (02)*: migrate copies of real historical databases and compare semantically, verify bundled SQLite version includes the WAL-reset fix.
- Hosted stores stay MySQL/PlanetScale (console, stats), S3/R2 objects and R2 SQL. Hosted adapters may use `mysql_async`/`sqlx` (choose one per store, record it in phase-12/13 contract).

## D6. HTTP and streaming

- Server: axum 0.8 on hyper 1, tower-http middleware. SSE and WebSocket via axum. Outbound: reqwest 0.13 with rustls.
- Route paths, methods, query coercion, defaults, status codes, error bodies, headers, CORS and SSE framing are byte-compatible with `packages/server` and `packages/opencode` legacy routes. *Blocking proof (06)*: recorded baseline route fixtures replayed against the Rust server.

## D7. Providers

- Rust-owned protocol implementations over reqwest (`responses`, `chat`, `messages`, `gemini`, `bedrock` families). No community "universal" SDK as authority. AWS SigV4 through `aws-sigv4`.
- Live provider calls need credentials the agents do not have. Deterministic recorded-stream fixtures are mandatory, live acceptance rows stay *blocked* with an owner.

## D8. Extensions

- MCP: rmcp for stdio and streamable HTTP, plus a Rust-owned legacy HTTP+SSE client (rmcp removed it). ACP: `agent-client-protocol` crate or hand-rolled JSON-RPC if the pinned crate does not match the baseline negotiation. LSP: `lsp-types` plus a Rust-owned JSON-RPC client.
- User JS/TS plugins: out-of-process compatibility host. The interpreter is the user's or a bundled Bun/Node treated as a third-party runtime, speaking a Rust-defined JSON-RPC ABI over stdio. Hook policy, ordering and permissions are Rust-owned. The bridge script in the host is generated from the Rust ABI definition.
- CodeMode: Rust-owned confined interpreter for the observed syntax subset, parsing with `oxc_parser`. No general JS engine.

## D9. User interfaces

- Terminal: ratatui 0.30 + crossterm 0.29, clap 4 for the CLI.
- Browser: Leptos 0.8 CSR compiled to `wasm32-unknown-unknown`, wasm-bindgen generated loader is an approved host binding. *Blocking proof (10)*: virtualized large timeline and streaming Markdown spike with recorded measurements.
- Desktop: Tauri 2 hosting the ui-web bundle, with Rust desktop-host owning every Electron main/preload operation. *Blocking proof (11)*: launch, local server spawn/stop, remote connect, deep link and updater spikes on macOS. Windows/Linux rows stay blocked until runners exist.

## D10. Hosted

- Hosted domain logic in `hosted-domain` (portable). HTTP services as native axum services in `bins/hosted` plus Cloudflare Worker targets through `worker` where the baseline runs on Workers. Infrastructure descriptions are generated by `xtask cloud` (Phase 13/15). No deploys.
- Stripe/mail/OAuth adapters are implemented and exercised against local test servers only. Live/staging rows stay *blocked* pending credentials and human approval.

## D11. Codegen and SDKs

- Rust protocol definitions are the single source. `xtask codegen` emits the OpenAPI document and the JavaScript/TypeScript SDK files. Generated JS is an approved binding, it carries no handwritten product logic. Byte-level comparison against the current generated SDK is the acceptance check.

## D12. Budgets

Provisional numeric targets are those in `docs/plans/rustport/production-gates.md` "Proposed measurable budgets". Phase 01/15 record the measured baseline values in `rust/contracts/phase-15/baselines.md`. Absolute ceilings remain *blocking proof* items for Phase 92.

## D13. Platform matrix

Agents can only execute on `aarch64-apple-darwin` (this machine) plus Docker `linux/arm64` containers. All other shipping rows (x86_64/arm64 Linux glibc/musl, Windows x64/arm64, macOS x64, all browsers/webviews other than those reachable here) are recorded as *blocked: runner unavailable* with the workflow ready to run.

## Change control

A crate owner who needs a new first-party edge, a schema/protocol change affecting another phase, or a reversal of a decision above sends a DM to the coordinator with: reason, evidence, affected crates/phases, wire or migration impact. The coordinator updates this record (bump the version line), `crate-edges.toml`, and notifies affected owners. Unrelated work continues meanwhile. Nobody invents alternate semantics locally.

## Coordinator verification log

- 2026-10-04 04:28: `opencode-runtime` composes the real stack (dedb72c52). `cargo test -p opencode-runtime` 4/4 and `cargo test -p opencode --test serve --test cli_help` pass. The `opencode serve` binary was smoke-tested in an isolated HOME: readiness line, `/api/location` resolves the git project id, `POST /api/session` creates and lists, SIGTERM exits 0, and the DB lives at `$HOME/.local/share/opencode/opencode-local.db`. Legacy `/session` returns 503 (legacy handlers are not composed yet, owner p06.legacy-registry).

- 2026-10-04 04:29, D14 (finding from p05.tools-exec): core `glob`/`grep` ask permission on the pattern only, then search an unvalidated `path` outside the Location without `external_directory` approval (baseline `packages/core/src/tool/glob.ts:62-78`, `grep.ts:77-97`). Decision: keep baseline parity in the port (test `search_tools.rs::finding_core_search_escapes_location_without_external_approval` pins it). List it in the Phase 92 dossier as a baseline security issue to fix in both implementations after cutover, not silently in the port only.
- 2026-10-04 00:58: `cargo test --workspace --no-fail-fast` (all crates, 186 test binaries): 1240 passed, 1 failed (same known `desktop-host` `proxy_env_resolution_and_bypass` in uncommitted p11.lifecycle work), 2 ignored. Includes the new codemode and opencode CLI spec suites.

- 2026-10-03 22:48: `cargo test --workspace --no-fail-fast` (excluding in-flight `integration-adapters` and `codemode`) across 166 test binaries: 925 passed, 1 failed, 2 ignored. The failure, `desktop-host/tests/lifecycle_config.rs::proxy_env_resolution_and_bypass`, is in uncommitted interrupted work of `p11.lifecycle.lifecycle` (lowercase `https_proxy` precedence over `HTTPS_PROXY` returns None for the https proxy_for lookup). That node owns the fix. `cargo check --workspace --all-targets` clean at 22:45.

- 2026-10-03 11:43: resolved first-party normal dependency edges (39 edges from `cargo metadata --no-deps`) all inside `crate-edges.toml`, no undeclared crates. Interim script check (bun), superseded by Phase 01 `cargo xtask edges` once it lands.
- 2026-10-03 11:44: `cargo check --target wasm32-unknown-unknown` passes for every portable crate present: schema, protocol, client, view-model, llm-api, storage-api, event-api, permissions, tool-api, session-api, platform-api, integration-api, hosted-domain. These are API/contract crates at this point, not complete implementations.
- 2026-10-03 11:42: `cargo check --workspace --all-targets` with 32 crates: one error, `sdk-embedded` declares a `launcher` module that was not yet written (owner mid-edit).

## Version log

- v2, 2026-10-03 22:03: event-api transaction contract fixed. Defect (reported by p04.admission): `Projector<Tx>`/`CommitHook<Tx>` with `EventStore::Tx = dyn Sql` resolve to `dyn Sql + 'static`, while `DatabaseExt::transaction` closures receive `&'a (dyn Sql + 'a)` borrowed from the connection, so no store can pass its transaction to projectors or hooks. Decision: event-api depends on storage-api (new allowed edge `event-api -> storage-api` in crate-edges.toml), drops the `Tx` associated type and generic `PublishOptions<Tx>`, and uses `Projector = Arc<dyn Fn(&dyn Sql, &Payload) -> Result<(), BoxError> + Send + Sync>` and `CommitHook = Box<dyn FnOnce(&dyn Sql, i64) -> Result<(), BoxError> + Send>`. Elided lifetimes are higher-ranked, matching `FnOnce(&dyn Sql)` in storage-api. Rejected: a `'static` transaction in storage-sqlite (needs unsafe or connection re-architecture) and a GAT transaction family (higher-ranked projections in `dyn Fn` are fragile). Implementer: p02.events worker. Affected consumers: event-store, session-runtime (projection.rs, todo.rs), context (epoch persistence), locations.

- v1, 2026-10-03 21:18: `LocationMutation.resolve` (baseline `packages/core/src/location-mutation.ts`) is a separate `LocationMutation` trait in `platform-api/src/fs.rs` beside `FileMutation`, not a `FileMutation::resolve` method. Native Location-root-scoped implementation in `platform-native/src/fs/`. Owner Phase 05 (supersedes the "LocationMutation [02]" note in phase-91/composition-notes.md). Consumers: tools read/write/edit/apply_patch/bash, config agent plugin.

- v0, 2026-10-03: initial provisional record.
