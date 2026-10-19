# API boundary implementation delivery, October 19, 2026

This supplements the earlier audit slice in `server-http-status.md`. It does not certify full product parity. The immutable starting census is 384 incompatible cases out of 430; no fixture, expected result, or ledger was changed by this API implementation.

## Baseline requirement → implementation → observation

| Baseline requirement | Implementation | Observed public-boundary result |
| --- | --- | --- |
| Full legacy config/project/path/file/find/VCS/command/agent/global/experimental/workspace/sync/TUI route families, exact methods and query declarations | `protocol/src/legacy/{build,core,experimental,workspace,tui}.rs` | `server/tests/http_domains.rs`: valid requests hit named service ports rather than UI fallback; required queries, enums and numeric bounds reject before dispatch |
| Provider credentials/OAuth and MCP route payload shapes | `protocol/src/legacy/integration.rs` | Actual loopback authorize requests distinguish raw-body BadRequest from schema BadRequest; invalid nested MCP config rejects |
| Permission/question/PTY/tool routes | `protocol/src/legacy/native.rs`, `server/src/handlers/legacy.rs` | Actual loopback enum/nested-array/PTY payload rejection; valid absent-service requests are explicit 503, never fabricated success |
| Remaining session mutations/history/parts routes and raw fork decoding | `protocol/src/legacy/session.rs`, `server/src/handlers/dispatch.rs` | Actual loopback valid fork raw bodies reach session adapter; malformed JSON, null, arrays and invalid IDs reject with baseline raw BadRequest |
| Named schema errors and decode order | Named Auth/Config/Part/Worktree definitions; `server/src/middleware/decode.rs`; `handlers/dispatch.rs` | Loopback exact Auth/Config/WorktreeRemoveInput messages, no-media empty shell body 400, sync nonempty tuple missing-key path |
| Real domain composition seams, not host-local domain emulation | `Services::{legacy_sessions,legacy_global,legacy_native,legacy_integrations,legacy_providers}` | Concrete adapters supplied by respective owners; absent composition remains typed operation 503 |
| Current API domain dispatch and envelopes | `handlers/{mod,legacy}.rs` | Current routes pass canonical `v2.*` operation IDs; raw adapter data wrapped with resolved location/data or data-only according to middleware; declared Void responses become empty 204 |
| Pagination status/headers and real request Link | Additive `LegacySessions::handle_response`, `LegacyResponse`, `LegacyError::Response`; `handlers/legacy.rs` | Host preserves response metadata and constructs Link from actual URI/Host; runtime strict test owner is adding persisted two-message cursor proof. Not yet independently accepted here |
| PTY ticket issuance and origin checks | `handlers/legacy.rs`, existing TicketStore | Checks x-opencode-ticket and configured origins; requires actual PTY lookup before issuing scoped token. Production PTY acceptance belongs to native/runtime composition |
| Deprecated permission response session lookup | `handlers/legacy.rs` | Actual session adapter lookup precedes pending permission response; runtime/native owners share the real registry |

## Verification observed so far

All Cargo invocations use the isolated repair-env wrapper, `--locked --offline`, and scoped packages.

- Scoped `cargo check -p opencode-server -p opencode-protocol`: passed.
- Scoped all-target Clippy with `-D warnings`: passed at 11:31 UTC, before the final named-schema improvements.
- Scoped package tests, explicitly excluding the separately measured known failing `baseline_replay_matches_ledger`: passed, including SSE and WebSocket suites. This exclusion is not a parity result, and the replay assertion remains unchanged.
- New actual loopback `http_domains`: all five tests passed at 11:33 UTC. These exercise the real listener and HTTP boundary with intentionally absent domain services. They do not certify domain success or production composition.
- First unmodified empty-host replay observation after mounting: **430 cases, 129 pass, 301 failures**, categorized **276 not-ported, 16 body, 5 status, 4 not-mounted**. This differs from the old ledger and the assertion fails, as it should. The 83 newly matching cases are request-boundary improvements, not 83 completed domain implementations.
- A later replay retry was blocked by an in-flight shared manifest/lock mismatch. Root owns lock refresh. Exact post-schema-fix census remains pending.

## Remaining blockers and proof needed

| Item | Missing behavior / uncertainty | Owner | Required proof |
| --- | --- | --- | --- |
| Production replay | All 430 cases must exercise concrete composed services, not empty Services | root + bug + domain owners | strict runtime replay without expected-failure exemptions |
| Remaining exact decoder messages | Unnamed union rendering, especially TUI publish; optional named Worktree payload label newly repaired and awaiting replay | API | immutable fixture comparison |
| Full payload/output schema parity | Config is currently record-shaped; Part has only common required fields; several arbitrary nested schemas and success outputs remain unknown | API + domain owners | full TS schema inventory and OpenAPI differential |
| Concrete per-family service behavior | Host wiring does not implement providers, integrations, permissions, sessions, filesystem, workspaces or TUI domains | assigned domain owners | successful and error-path loopback tests over actual stores/services |
| Remote WebSocket proxy | Existing remote-workspace PTY upgrade relay remains unsupported | API/root | actual bidirectional remote relay acceptance |
| Other payload encodings | Non-JSON endpoint payload decoding remains unsupported where declared | API | per-encoding baseline fixtures |
| Fence/config error mapping and UI assets | Earlier phase blockers remain unless composed and tested by their owners | root/config/web owners | exact error/fence/document/UI acceptance |
| Final strict Clippy and replay after final decoder fixes | Shared lock refresh required | root then API | locked offline scoped checks and exact remaining failure report |

No production user data, TypeScript baseline source, manifests, crate edges, expected-failure ledger or fixtures were edited in this delivery.
