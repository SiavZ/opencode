# Phase 07 inventory

Baseline `907b3bc518fa48e90e8ec24dd327d13eee71c36c`. Per-area detailed inventories (with line ranges) live in `mcp.md`, `lsp.md`, `acp.md`, `plugins.md`, `codemode.md`, written by the owning child nodes.

| Area | Baseline source | Rust owner |
| --- | --- | --- |
| MCP config schema | packages/core/src/v1/config/mcp.ts:1-63 | integration-api/src/mcp.rs |
| MCP status union, Interface | packages/opencode/src/mcp/index.ts:83-200 | integration-api/src/mcp.rs |
| MCP catalog sanitize/paginate | packages/opencode/src/mcp/catalog.ts:11-30,117 | integration-api/src/mcp.rs (constants), adapters mcp/ |
| MCP transports, OAuth, callback | packages/opencode/src/mcp/{index,auth,oauth-provider,oauth-callback,browser}.ts | integration-adapters/src/mcp/ |
| LSP config schema, 38 builtin ids | packages/core/src/v1/config/lsp.ts:1-80 | integration-api/src/language.rs |
| LSP values, Interface | packages/opencode/src/lsp/lsp.ts:18-134 | integration-api/src/language.rs |
| LSP diagnostic format | packages/opencode/src/lsp/diagnostic.ts:1-29 | integration-api/src/language.rs |
| LSP servers/client/launch | packages/opencode/src/lsp/{server,client,launch,language}.ts | integration-adapters/src/lsp/ |
| ACP agent | packages/opencode/src/acp/*.ts (12 modules), cli/cmd/acp.ts | integration-adapters/src/acp/ |
| Plugin hook surface | packages/plugin/src/index.ts:222-334, tool.ts | integration-api/src/plugin.rs |
| Plugin trigger ordering | packages/opencode/src/plugin/index.ts:284-297 | integration-api/src/plugin.rs `trigger` |
| Plugin loading/host | packages/opencode/src/plugin/{index,loader,shared,install,meta}.ts | integration-adapters/src/plugins/, bins/compatibility-host |
| First-party auth plugins | packages/opencode/src/plugin/{azure,cloudflare,...}.ts | out of scope: Phase 03 |
| CodeMode | packages/codemode/src/** | crates/codemode |
