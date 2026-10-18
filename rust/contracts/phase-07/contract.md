# Phase 07 contract: extension hosts and external protocols

Status: v0 (provisional, 2026-10-03). Owner: Phase 07. Decisions: phase-90 D8.

## Crates

| Crate | Target | Role |
| --- | --- | --- |
| `opencode-integration-api` | portable | MCP, language and plugin-hook values plus service traits. No I/O. |
| `opencode-integration-adapters` | native | `mcp/`, `lsp/`, `acp/`, `plugins/` adapters implementing the API traits. |
| `opencode-codemode` | portable | Confined CodeMode interpreter (oxc_parser), host tool bridge, OpenAPI generation. |
| `opencode-compatibility-host` (bin) | native | Spawns bun/node as a third-party interpreter for user JS/TS plugins over a Rust-defined JSON-RPC stdio ABI. |

## integration-api (consumers: 04 session-runtime, 05 tools, 06 server, 08 SDK, 14 ecosystem)

- `mcp`: `ServerConfig` (`McpLocalConfig`/`McpRemoteConfig`, `type` discriminator, `oauth: false` literal), `Status` (`MCPStatus`, `status` discriminator, snake_case), `AuthStatus`, `ToolDef` (unknown spec fields preserved), `McpTool`, `ServerInstructions`, `Resource`, `ResourceTemplate`, `Prompt`, `McpEvent` (`mcp.tools.changed`, `mcp.browser.open.failed`), `sanitize_name` (`/[^a-zA-Z0-9_-]/g -> _`), constants `DEFAULT_TIMEOUT_MS=30000`, `MAX_LIST_PAGES=1000`, callback `127.0.0.1:19876/mcp/oauth/callback`. Trait `McpService` mirrors `MCP.Interface` (packages/opencode/src/mcp/index.ts:164-200) plus `call_tool`, the single MCP call path shared by direct tools and CodeMode children.
- `language`: `LspConfig` (`bool | Record<id, entry>`, custom ids need `extensions`), `BUILTIN_SERVER_IDS` (38, baseline order), `Position` (UTF-16 code units), `Range`, `Symbol`, `DocumentSymbol`, `Status` (`LSPStatus`), `Diagnostic` with byte-compatible `pretty`/`report` (20 severity-1 issues per file). Trait `LanguageService` mirrors `LSP.Interface` (packages/opencode/src/lsp/lsp.ts:119-134) plus `shutdown` that joins processes.
- `plugin`: `TriggerName` (all 15 `(input, output)` hooks of packages/plugin/src/index.ts:222-334 with exact names), `HookCapabilities`, `CustomTool`, `ToolContext` (`sessionID`/`messageID` casing), `ToolResult` (string or structured), trait `PluginInstance`, and `trigger()` the Rust-owned ordering policy: load order, sequential, output threaded, first error aborts. `permission.ask` has no baseline trigger site and is never a policy override.
- `error::IntegrationError` keeps not-found / failed / timeout / cancelled / validation / permission-denied / closed / `NotYetPorted(&'static str)` distinct. `is_retryable` = timeout or closed.

## Adapter contracts (owned by p07 children, detailed in `mcp.md`, `lsp.md`, `acp.md`, `plugins.md`, `codemode.md`)

- MCP: per-Location state, Streamable HTTP then legacy SSE fallback for remote, stdio for local, OAuth credentials bound to exact URL, pending transport per name with cancellable callback wait, catalog pagination guards.
- LSP: `(server, root)` dedup, broken set, Content-Length JSON-RPC, document versioning, push/pull diagnostics, joined shutdown.
- ACP: NDJSON stdio agent, backend reached through a trait (session-api not yet published), per-session permission queue.
- Plugins: Rust owns load order and hook policy, foreign code only inside the compatibility host. Bridge script generated from the Rust ABI.
- CodeMode: no ambient process/network/fs, data-only boundary, sanitized errors, explicit limits.

## Conformance tests

`cargo test -p opencode-integration-api -p opencode-integration-adapters -p opencode-codemode -p opencode-compatibility-host`. Fixture vs live labels are recorded per area in `status.md`.
