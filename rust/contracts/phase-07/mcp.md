# Phase 07 MCP client contract (`p07.mcp`)

Owner: p07.mcp. Crate: `opencode-integration-adapters`, module `src/mcp/`. Implements
`opencode_integration_api::mcp::McpService` (`McpServiceImpl`). Updated 2026-10-03.

## Implemented

| Area | Rust | Baseline |
| --- | --- | --- |
| Service state machine: lazy concurrent connect of configured servers, status union (`connected`, `disabled`, `failed`, `needs_auth`, `needs_client_registration`), clients/defs/instructions maps, `status` key order (static config then runtime adds, missing = disabled) | `service.rs`, `service_api.rs` | `index.ts:492-660` |
| `add` / `connect` (forces `enabled: true`) / `disconnect` (status `disabled`) / reconnect via `connect`, previous client closed on replace | `service.rs`, `service_api.rs` | `index.ts:562-659` |
| Watchers: connection close -> `failed: Connection closed` + `mcp.tools.changed`; `notifications/tools/list_changed` -> re-list (only while the client is still current and connected) + `mcp.tools.changed`; logging notifications routed by level | `service.rs::watch`, `client.rs::server_log` | `index.ts:442-490` |
| Shutdown: SIGTERM stdio descendants found by breadth-first `pgrep -P`, close clients, join watcher tasks, drop pending OAuth flows | `service.rs::shutdown`, `stdio.rs::descendants` | `index.ts:418-440, 531-556` |
| Request timeout resolution: runtime config, static config, `experimental.mcp_timeout`, then SDK 60 s default; connect timeout `timeout ?? 30000` | `service.rs::request_timeout`, `connect.rs` | `index.ts:661-664, 286, 359` |
| JSON-RPC peer: id correlation, timeouts send `notifications/cancelled`, progress tokens reset the timeout for `tools/call`, server requests `roots/list` (single root = Location dir as `file://` URL) and `ping`, unknown requests -32601, pending requests fail `MCP error -32000: Connection closed` on close | `jsonrpc.rs`, `client.rs` | SDK `Protocol`, `index.ts:75-81` |
| Initialize: protocol `2025-11-25`, supported list, `roots` capability only, instructions trimmed, `notifications/initialized` | `client.rs` | SDK `Client._initialize` |
| stdio transport: command/args, `cwd` resolved against Location dir, env = sudo-style defaults + process env + `BUN_BE_BUN=1` for `opencode` + config env, stderr drained, NDJSON with `\r` strip, close = stdin EOF, 2 s, SIGTERM, 2 s, SIGKILL; `spawn X ENOENT` errors | `stdio.rs`, `connect.rs::connect_local` | `index.ts:340-370`, SDK `stdio.js` |
| Streamable HTTP transport: POST JSON or SSE responses, `mcp-session-id`, `mcp-protocol-version`, optional GET listen stream (405 tolerated), user headers override generated ones, 401 -> OAuth then single retry (circuit breaker), 403 `insufficient_scope` upscoping once per header | `streamable.rs`, `remote.rs` | SDK `streamableHttp.js` |
| Legacy HTTP+SSE transport (Rust-owned, rmcp has none): GET stream, `endpoint` event with same-origin check, POST to endpoint, 401 -> OAuth retry, stream end closes the connection | `legacy_sse.rs`, `sse.rs` | SDK `sse.js` |
| Remote connect: Streamable HTTP then SSE fallback, auth errors stop the fallback, `needs_client_registration` when the auth error mentions `registration`/`client_id`, otherwise `needs_auth` with the pending transport kept for `finish_auth`; warning toasts with baseline text | `connect.rs::connect_remote` | `index.ts:236-338` |
| OAuth protocol (SDK `auth()`): RFC 9728 path-aware then root discovery, RFC 8414/OIDC candidates, resource selection, scope determination (+ `offline_access`), dynamic client registration, PKCE S256 (43-char verifier), state, code exchange, refresh with server_error tolerance, client auth method selection (basic/post/none), invalid_client/invalid_grant retry with invalidation | `oauth.rs`, `oauth_discovery.rs`, `oauth_token.rs` | SDK `auth.js` |
| OAuth provider: redirect URL (`redirectUri` > `callbackPort` > 19876), client metadata, persistent vs pending (commit) credentials, exact-URL credential binding, secret expiry, state generator, invalidation | `oauth_provider.rs` | `oauth-provider.ts` |
| Token store `mcp-auth.json` under injected data dir, mode 0600, file lock + in-process mutex, invalid file reads `{}`, JS number formatting | `auth_store.rs` | `auth.ts` |
| Callback server `127.0.0.1:<port><path>`: 404 other paths, missing state 400, unknown state 400, `error`/`error_description` reject, timeout 5 min, `cancel_pending(name)`, stop when idle, restart on port/path change, no-op when the port is already bound | `oauth_callback.rs` | `oauth-callback.ts` |
| `start_auth` / `authenticate` (wait registered before browser opens, `mcp.browser.open.failed` on launcher error, CSRF state re-check) / `finish_auth` / `remove_auth` / `supports_oauth` / `has_stored_tokens` / `auth_status` (expired by `expiresAt`) | `service_auth.rs`, `service_api.rs`, `browser.rs` | `index.ts:806-970`, `browser.ts` |
| Catalog: pagination with duplicate-cursor and 1000-page guards, tolerant `tools/list` page (unresolvable/non-object outputSchema strips outputSchema from the page), capability-gated prompts/resources/templates, keys `sanitize(client):sanitize(name)` and `escaped(client):uri`, `tool_name` | `catalog.rs` | `catalog.ts` |
| `call_tool` result translation: `isError` -> error joined from non-blank text parts (`MCP tool returned an error` fallback), structured-only -> JSON text block, outputSchema without structuredContent -> -32600 error; errors map to typed `IntegrationError` (timeout, cancelled, closed, failed) | `catalog.rs::translate_call_result`, `service_api.rs` | `catalog.ts:42-83`, SDK `callTool` |

## Tests

All fixture (local processes and loopback peers); none are live.

| Test | Covers |
| --- | --- |
| `tests/mcp_stdio.rs::fixture_stdio_catalog_and_calls` | real Python stdio server: 2-page tools list, sanitized names, tolerant outputSchema page, instructions trim, echo/error/structured calls, env + cwd, `roots/list`, prompts/resources/templates keys, getPrompt/readResource, list_changed refresh + event |
| `tests/mcp_stdio.rs::fixture_stdio_timeouts_progress_and_close` | timeout -> `Timeout { ms }` + `notifications/cancelled`, progress resets timeout, server exit -> failed `Connection closed`, reconnect, disconnect, NotFound |
| `tests/mcp_stdio.rs::fixture_stdio_add_failures_and_guards` | ENOENT spawn status, duplicate cursor -> `Failed to get tools`, disabled add, connect forces enable, relative cwd |
| `tests/mcp_stdio.rs::fixture_stdio_shutdown_kills_descendants` | grandchild process SIGTERMed on shutdown |
| `tests/mcp_stdio.rs::fixture_pagination_guards` | 1000-page guard |
| `tests/mcp_http.rs` (3 tests) | axum Streamable HTTP peer (JSON + SSE responses, session id, protocol version, custom headers), legacy SSE fallback with `oauth: false`, invalid URL, unreachable host, SSE stream end -> failed |
| `tests/mcp_oauth.rs` (4 tests) | local OAuth AS: needs_auth + toast, full authenticate (RFC 9728/8414, DCR, PKCE verified by the AS, state, callback, token persisted 0600 bound to URL), needs_client_registration, `oauth` without registration endpoint, browser failure event + cancel, callback server validation/timeout, store semantics |
| unit tests in `sse.rs`, `catalog.rs` | SSE framing, keys, call translation, tolerant schema, output-schema contract |

Run: `cargo test -p opencode-integration-adapters --test mcp_stdio --test mcp_http --test mcp_oauth` (needs `python3` and `pgrep` on PATH).

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| callback page markup | byte-identical branded HTML from `packages/core/src/oauth/page.ts` (Rust serves a minimal page with the same status codes and messages) | the shared page renderer is a core asset not yet ported to Rust | p07.mcp / core owner | port `OauthCallbackPage` once and snapshot-compare |
| streamable session recovery | SDK re-initializes on HTTP 404 for an expired `mcp-session-id` and retries once | not exercised by opencode code paths directly; today the request fails with `Streamable HTTP error: Error POSTing to endpoint: ...` | p07.mcp | fixture peer returning 404 after a session id, then assert transparent retry |
| SSE resumption | `last-event-id` reconnect and server `retry` backoff for streamable listen/response streams | reconnect loop not ported; streams end the request or listen channel | p07.mcp | peer that drops mid-stream with event ids |
| full JSON Schema output validation | SDK validates `structuredContent` against `outputSchema` (Ajv); Rust only enforces presence and the tolerant `$ref`/`type` checks | `jsonschema` not yet a dependency of this crate | p07.mcp | add validator, test invalid structured content -> `-32602` |
| task-based tools | `execution.taskSupport: required` tools rejected by `callTool` | baseline never enables tasks; low value | p07.mcp | n/a until tasks are enabled |
| bus / toast wiring | `McpEvents` is an injected trait; composition with EventV2 bridge and `TuiEvent.ToastShow` | runtime composition (phase 04/06) consumes this service | runtime owner | composed server emits `mcp.tools.changed` |
| Windows | `descendants` returns `[]`, permissions not set | baseline does the same on win32; no Windows CI | p07.mcp | n/a |

No `IntegrationError::NotYetPorted` path is needed. Every `McpService` method has a real implementation.

## Inventory (baseline line ranges)

| Baseline | Lines | Rust |
| --- | --- | --- |
| `mcp/index.ts` createClient, status schema | 38-107 | `client.rs`, `integration-api/src/mcp.rs` |
| `mcp/index.ts` connectTransport, connectRemote, connectLocal, create | 218-415 | `connect.rs` |
| `mcp/index.ts` descendants, watch, serverLog | 418-490 | `stdio.rs`, `service.rs`, `client.rs` |
| `mcp/index.ts` state, finalizer, closeClient, storeClient | 492-589 | `service.rs` |
| `mcp/index.ts` status, instructions, add, connect, disconnect, tools, collect, getPrompt, readResource | 591-804 | `service_api.rs`, `service.rs` |
| `mcp/index.ts` startAuth, authenticate, finishAuth, removeAuth, supportsOAuth, hasStoredTokens, getAuthStatus | 806-970 | `service_auth.rs`, `service_api.rs` |
| `mcp/catalog.ts` | 1-170 | `catalog.rs` |
| `mcp/auth.ts` | 1-163 | `auth_store.rs` |
| `mcp/oauth-provider.ts` | 1-263 | `oauth_provider.rs` |
| `mcp/oauth-callback.ts` | 1-194 | `oauth_callback.rs` |
| `mcp/browser.ts` + `core/open.ts` | 1-40, 1-8 | `browser.rs` |
| SDK `client/auth.js` (auth, discovery, registration, tokens) | 146-930 | `oauth.rs`, `oauth_discovery.rs`, `oauth_token.rs` |
| SDK `client/streamableHttp.js`, `client/sse.js`, `client/stdio.js` | full | `streamable.rs`, `legacy_sse.rs`, `sse.rs`, `stdio.rs` |
