# Phase 06 / p06.server-http status

Owner node: p06.server-http. Crate: `rust/crates/server` (HTTP core). Commits: 699862e39,
9b1ec9327, 4987d4f39 (plus the final status commit).

## Public constructors (for p06.fixtures, p08 sdk-embedded, p91 runtime)

- `opencode_server::app::router(services, ServerConfig) -> axum::Router`: full host
  (`HttpApiApp.createRoutes`): every `opencode_protocol::registry()` operation, `/doc`, UI
  fallback, CORS, compression, auth.
- `opencode_server::app::embedded_router(services)`: `createEmbeddedRoutes`: current API only, auth
  off, `/openapi.json`.
- `opencode_server::app::current_router(services, password)`: `createRoutes(password)`.
- `opencode_server::app::router_with(&[&Api], services, ServerConfig)`: explicit API list for
  fixture replay and contract tests.
- `opencode_server::lifecycle::{listen, serve, Listener}`: bind-before-ready, port 0 prefers 4096,
  `readiness_line()`, `stop(close)` joins every connection task.

## Implemented

| Area | Where | Tests |
| --- | --- | --- |
| Registry mounting (`:p` -> `{p}`, `*` -> `{*wildcard}`, shared method routers) | `src/app.rs` | `tests/http_errors.rs` |
| Unported operation -> 503 `ServiceUnavailableError` with `service: <operation>` | `src/middleware/errors.rs`, `src/handlers/mod.rs` | `tests/http_errors.rs` |
| Decode Params, Query (bracketed deepObject, NumberFromString), Payload; current tagged vs legacy NamedError rejection; 415 media type | `src/middleware/decode.rs`, `src/handlers/dispatch.rs` | `tests/http_session.rs` |
| Auth: query token precedence, username default, empty password disables, WWW-Authenticate, PTY ticket bypass, public UI assets, `/api/` tagged vs empty 401 | `src/middleware/auth.rs` | `tests/http_auth.rs` |
| CORS allowlist, preflight before auth, Vary merge | `src/middleware/cors.rs` | `tests/http_cors.rs` |
| Location and session-location middleware | `src/middleware/location.rs` | `tests/http_session.rs` |
| Legacy workspace routing + remote HTTP proxy | `src/middleware/legacy_routing.rs`, `src/transport/proxy.rs` | `tests/http_proxy.rs` |
| Compression (full host) | `src/middleware/compression.rs` | unit tests |
| Health, location, session list/create/active/get/prompt/compact/wait/revert/context/history/interrupt/message, messages paging | `src/handlers/{mod,session,cursor}.rs` | `tests/http_session.rs` (fixture SessionService) |
| Lifecycle | `src/lifecycle.rs` | `tests/http_lifecycle.rs` |

All socket tests use fixture ports from `tests/support/mod.rs` (fixture-only, not adapters).

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| session/message groups in `opencode_protocol::current` | registry only has health + location; session endpoints are mounted only via `router_with` in tests | p06.current-registry not done | p06.current-registry | `tests/http_errors.rs` iterates the real registry including session endpoints with `Middleware::SessionLocation` |
| legacy registry | `opencode_protocol::legacy::api()` is empty, so no legacy route is mounted on the full host | p06.legacy-registry not done | p06.legacy-registry | full-host router mounts legacy groups with `Middleware::LegacyInstance` |
| `LocationResolver`, `SessionPlacement`, `LegacyWorkspaces` ports | narrow port traits in `src/services/ports.rs`; no production adapter | `opencode-locations` is a skeleton | Phase 02 (p02.locations) | adapter implementing the traits, wired in runtime |
| `UiAssets` port | UI fallback answers 503 without it | Phase 10 web UI not composed | Phase 10 | adapter + `GET /` test |
| `/doc`, `/openapi.json` body | serves `opencode_protocol::openapi::document`, currently a stub `{openapi: "3.1.0"}` | p06.events-openapi | p06.events-openapi | byte-compare with `packages/sdk/openapi.json` |
| WebSocket proxy to remote workspace | answers 503 NotYetPorted | not implemented | p06.server-http follow-up | upgrade relay test |
| multipart/form/text/bytes payloads | answer 503 NotYetPorted | only JSON decoding implemented | p06.server-http follow-up | per-encoding decode tests |
| fence header (`OPENCODE_WORKSPACE_ID`) | not implemented | needs Database port | p06 + Phase 02 | fence diff test |
| config error 400 mapping (`ConfigErrorV1`) in legacy error layer | defects always map to UnknownError 500 | Phase 02 config errors not exposed | Phase 02 | legacy error test |

## p06.server-streams

Commits 53c6e934d, 0bb16945a. Routes mounted by `handlers::streams::routes()` (the generic dispatcher
skips them via `owns(path)`): `/api/event`, `/api/session/:sessionID/event`, `/api/pty/:ptyID/connect`,
`/event`, `/global/event`, `/pty/:ptyID/connect`.

Ports (`src/services/streams.rs`): `EventSource` is blanket-implemented for the event-api `EventStore`,
so no new upstream trait is needed. Also `GlobalBus` (legacy bus), `PtyProvider` (location-scoped
platform-api `PtyService`), `PtyTickets` with the in-process `TicketStore` (60 s TTL, 10_000
entries, atomic scoped consume), and `SocketTracker` (1001 `server closing`, joined).

Tests: `tests/sse_events.rs` (8) and `tests/ws_pty.rs` (7) are fixture-based socket tests with in-memory
event, bus and PTY adapters.

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| socket shutdown wiring | `lifecycle::Listener::stop` does not call `services.streams.sockets.close_all()`; the host must call it before `stop` | lifecycle is router-agnostic | p91 runtime | runtime shutdown test |
| `pty.connectToken` handlers | ticket issuing endpoints (current + legacy) still answer 503; `handlers::streams::issue_ticket` is ready | PTY CRUD handlers not ported | p06 follow-up | connect-token socket test |
| GlobalBus / PtyProvider adapters | no production adapter | Phase 02 / Phase 05 composition | p91 runtime | live test |
| stream failure body | `/api/event` overflow aborts the body (no failure frame), like an Effect raw stream failure; `/api/session/:id/event` errors write `effect/httpapi/stream/failure` with a generic Die cause | exact Effect defect JSON for store errors not captured | p06.fixtures | byte fixture |
| WebSocket remote-workspace proxy for `/pty/:id/connect` | answers 503 | not implemented | p06 follow-up | relay test |
