# Runbook: credential rotation

Credentials in scope:

| Credential | Where it lives | Rotated by |
| --- | --- | --- |
| Provider API keys / OAuth tokens (user) | `$XDG_DATA_HOME/opencode/auth.json`, mode 0600 (`packages/opencode/src/auth/index.ts:10,79,88`), or env vars | the user |
| MCP server OAuth tokens | `packages/opencode/src/mcp/auth.ts` store | the user |
| Server basic-auth password (`OPENCODE_SERVER_PASSWORD`) | process env / container env | the operator |
| OTLP collector headers | `OTEL_EXPORTER_OTLP_HEADERS` | the operator |
| Release signing (Apple cert, Azure Trusted Signing), GitHub tokens | GitHub environment secrets (baseline `publish.yml`) | the release owner (Phase 94) |

## Procedure (any credential)

1. **Issue new before revoking old**, when the provider allows two active keys.
2. Install the new credential:
   - User provider key: `opencode auth login <provider>` (replaces the entry in `auth.json` and keeps it 0600), or update the env var.
   - Container: update the secret in the orchestrator (never in the image or a `docker run -e` in shell history), then restart the container.
   - CI secrets: update the GitHub environment secret, then re-run a dry `workflow_dispatch` that does not publish.
3. **Revoke the old credential** at the provider.
4. **Verify**: one authenticated request succeeds with the new credential, and a request forced to use the old one fails with 401/403.
5. Record what was rotated, when, and why. For a suspected leak, also open an incident (`incident-response.md`) and check provider audit logs for use between exposure and revocation.

## Leak specifics

- `auth.json` committed or attached to an issue: rotate **every** entry in that file, not only the one noticed.
- A secret in a container layer: rebuild without it, rotate, and treat the old image digest as compromised (purge it from registries).
- Check `ls -l auth.json` is `-rw-------` after rotation. A wider mode is itself a finding.

## What is untested

No rotation has been exercised against the Rust port. `opencode auth login` is not ported yet (Phase 09), the server password and OTLP header paths depend on unported server/observability code, and CI secret rotation needs the real repository settings. File mode 0600 is baseline behavior, not verified for Rust.
