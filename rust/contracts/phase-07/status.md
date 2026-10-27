# Phase 07 integration runtime status

Updated 2026-10-19. This is a partial parity delivery, not an all-integrations-complete claim.

## Concrete implementation

- Native MCP stdio/HTTP/OAuth and LSP clients are composed by the production Location bundle. MCP startup and request deadlines remain distinct, transport children receive the explicitly supplied Location environment, and native process shutdown is owned by the Location finalizers.
- `runtime/src/location_integrations.rs` owns actual external legacy-plugin interpreter processes, ordered plugin callbacks, registered plugin/MCP/resource/skill tools, configuration callbacks, skill/reference snapshots, and their lifetimes. Both native registry generations retain independent Location registrations. Dynamic MCP catalog reconciliation updates both registries without publishing Location tools into the process-wide application table.
- `LocationCallbacks` routes active tool authorization and progress through actual invocation contexts. Its startup `config.get` reads real merged configuration without recursively acquiring an unfinished Location. Other recursive startup SDK calls fail explicitly rather than deadlocking. Post-start SDK calls use the production service router.
- `IntegrationMaterializer` runs plugin before/after tool hooks around the actual native materializer. Modified output is rebound through that materializer's original registry and output store; retained artifact paths are preserved.
- `IntegrationContext` combines registered core context with agent-filtered skills, reference descriptions, and instructions from connected MCP servers. MCP source keys safely encode arbitrary server names.
- The actual built-in `customize-opencode` producer embeds the existing TypeScript source markdown. Canonical and legacy skill projections preserve their different locations and descriptions. User skill discovery can override the builtin.
- Remote skill discovery uses real indexed HTTP downloads, retries, safe cache paths, versioned staging, and failed-refresh preservation. Reference materialization uses actual native Git clone/fetch/reset and branch-isolated caches. Reference jobs are scoped and cancelled/joined during integration teardown; failed materialization logs instead of failing Location construction.
- MCP route/CLI ports expose actual transport state, capabilities/resources, authorization operations, and secret-free stored OAuth inventory, including no-longer-configured names. Lifecycle routes reconcile the registered catalog before returning.

## Validation already observed

All commands run from `rust/`, through the shared repair environment, with `--locked --offline` and package scopes.

- `cargo test -p opencode-integration-api -p opencode-integration-adapters -p opencode-codemode`: **317 tests passed**, including native MCP stdio/HTTP/OAuth, real LSP processes, generated JS bridge callbacks, local Git clone/refresh/branch isolation, controlled HTTP skill-index retry/version preservation, and the existing codemode interpreter suites.
- `cargo clippy -p opencode-integration-api -p opencode-integration-adapters -p opencode-codemode --all-targets -- -D warnings`: **passed**.
- Production SQLite event-store fixture `runtime/tests/integration_events.rs`: **passed**, proving a held plugin event callback cannot retain shutdown and the owned event pump joins.
- `runtime/tests/location_integrations.rs` exercises the real Process and Location graph, not an injected builtin-only host. It covers actual external plugin bootstrap SDK callbacks, permissions, native tool calls, MCP resource reads, skill/reference/MCP guidance, LSP startup, and joined disposal. Its final result is recorded below after the acceptance rerun.

Commits already made: `7f8dda434` plugin process host, `15889902d` HTTP skills/native Git references, `ae4915bd9` transport environment/deadlines and structured RPC errors.

## Exact remaining gaps

| Boundary | Missing behavior / evidence | Observable result / next proof |
| --- | --- | --- |
| Current Promise and Effect plugins | The external bridge currently accepts legacy function/server-hook factories, not `setup(ctx)` or `effect(ctx)` registration/reload domains from `packages/plugin/src/v2`. | Current factories are rejected, not silently loaded. Full domain-transform/runtime-hook compatibility and scoped registration disposal still require implementation and real fixtures. |
| Plugin config mutation | The legacy bridge calls the real `config` hook but its returned wire result does not propagate in-place JS configuration edits back into all downstream Location registries. | Callback invocation alone is not configuration-mutation parity. |
| Plugin workspace callbacks | `workspace.register` is not backed by the actual workspace adapter registry. | Unsupported callback remains explicit; no successful fake registration is claimed. |
| Bootstrap SDK breadth | During Location construction only real `config.get` is directly available. Other recursive SDK calls receive an explicit closed/bootstrap error. | Agent/catalog/native bootstrap primitives need direct ports to reproduce the full SDK surface without recursive Location deadlock. |
| External plugin package resolution | Native host runs explicitly resolved local modules with an explicitly available interpreter. Package installation/resolution is not implemented. | A package reference returns `NotYetPorted`; no install side effects occur. |
| Plugin server URL | Legacy plugin input currently uses the compatibility localhost URL rather than a live dynamically bound Process listener URL. | No claim of network SDK URL parity. Local SDK callback routing is real. |
| Codemode runtime composition | The real codemode interpreter/enumeration/promise/OpenAPI suites pass, but the production Location tool graph does not yet expose a codemode execution tool/registration. | Passing crate tests do not prove end-user runtime composition. |
| ACP | `integration-adapters/src/acp/mod.rs` remains a documentation-only module. | CLI owner reported no concrete ACP stdio protocol service. |
| Formatter | No native formatter inventory/process service is currently composed. | `formatter.status` remains explicitly unavailable, not an invented empty list. |
| Canonical integration authentication | `/api/integration` represents provider/plugin registry, connection and authorization-attempt APIs, not MCP. | Provider/auth owner is implementing the real seam; these endpoints must not be mapped to MCP as a shortcut. |
| Mixed-generation configuration | Flat legacy MCP-only configuration can be rejected by current-config decoding when no V1 discriminator key is present. | Root/config owner has the exact diagnosis; executable disabled-server fixture is the required proof after correction. |
| Rich plugin/MCP outputs | Legacy plugin structured attachments and embedded MCP binary-resource/tool-result formatting are not fully matched in every variant. | Current text/file paths are real; exact rich-content schema and presentation parity still need additional fixtures. |

## Final composed acceptance

Pending actual rerun. Do not interpret the adapter suite or a compiling constructor as a replacement for the production Process fixture.
