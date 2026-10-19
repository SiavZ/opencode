# Phase 06 / p06.server-http status

Owner node: p06.server-http. Crate: `rust/crates/server` (HTTP core). Commits: 699862e39,
9b1ec9327, 4987d4f39 (plus the final status commit).

## October 19, 2026 audit blocker repair

The full-host legacy registry now mounts the workflow-critical global, event and Session
boundaries rather than falling through to the UI. Global health returns the installation
version supplied by the process. Legacy Session responses use the V1 JSON shape, not the
current API's `{data: ...}` envelope or its `location` field.

- `GET /global/health`, `GET /global/event`, `GET /event`.
- `GET /session`, `POST /session`, `GET /session/status`, `GET /session/:sessionID`.
- `GET /session/:sessionID/message`, `GET /session/:sessionID/message/:messageID`,
  `POST /session/:sessionID/abort` dispatch through the same composed legacy boundary.
- `Services::legacy_sessions` is the explicit V1 runtime seam. Its implementation must use
  real persistence and projectors. The HTTP layer never synthesizes persisted Sessions or
  publishes duplicate `session.created` events. Absent composition is a typed operation 503.
- Create decodes its raw optional body like the TypeScript `createRaw` function, including
  whitespace-only bodies, no required JSON media type, and empty `_tag: BadRequest` 400
  responses for invalid JSON or invalid CreateInput. The complete model, metadata,
  workspace and permission input fields are represented in the registry.
- Instance directory decoding and canonicalization are shared by HTTP dispatch and SSE,
  including symlink identities, lexical dot segments and the ENOENT absolute-path fallback.
- Legacy ID validation happens before endpoint decoding, preserving the baseline's 500
  UnknownError for malformed Session/workspace route IDs. Missing valid IDs map to V1
  NotFoundError 404 through the runtime service.

New `tests/http_legacy.rs` exercises actual loopback listeners, Basic auth, CORS/preflight,
health/version, typed absent-service errors, and raw payload/query errors without source
mocks. `tests/legacy_location.rs` exercises the real temporary filesystem. Existing
`tests/sse_events.rs` remains fixture-backed transport coverage. Production persistence,
real event forwarding and restart proof belongs to the process-root tests in
`runtime/tests` and the shipped acceptance workflows, coordinated with Phase 91.

The empty-host baseline replay was regenerated after reviewing the route changes. It now
records 46 passing cases and 384 expected failures out of 430, not full API conformance.
Those failures remain explicit: an empty `Services` host has no domain implementations;
legacy families outside the listed routes are not advertised as implemented.

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

Socket auth/proxy/current Session tests use fixture ports from `tests/support/mod.rs`.
The new legacy HTTP tests use absent domain services intentionally, while filesystem
normalization tests use real directories. Neither substitutes for runtime persistence proof.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Remaining legacy families | only global health/events, directory events and the listed Session boundaries are registered; config/project/provider/file/permission/question/PTY CRUD and other Session mutations still require real adapters | not covered by the audited workflow repair | p06 + domain owners | end-to-end fixture/live proof for each family before advertising it |
| Legacy Session runtime composition | the server exposes `LegacySessions`; full-host persistence/event/restart proof must exercise the actual process adapter, not an injected source mock | HTTP layer cannot own runtime adapters | Phase 91 | process-root loopback tests + serve-health/session-create-list acceptance |
| `UiAssets` port | UI fallback answers 503 without it | Phase 10 web UI not composed | Phase 10 | adapter + `GET /` test |
| OpenAPI full baseline parity | schema emission is implemented, but remaining unregistered legacy families prevent full SDK document parity | remaining domain HTTP families are not ported | p06/domain owners | per-family schema fixtures and complete baseline document comparison |
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
