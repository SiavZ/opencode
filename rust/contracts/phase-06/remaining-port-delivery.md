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

## Follow-up: real remote WebSocket relay

The remote-workspace upgrade path now uses `server/src/transport/proxy_socket.rs`, rather than returning Unsupported. It upgrades the downstream connection, connects the routed upstream URL with the offered subprotocols, forwards text/binary/control frames, preserves remote close code/reason, and closes both peers on tracked shutdown. There are no detached pumps or application queues; socket writes apply backpressure in one owned task.

`server/tests/ws_proxy.rs` passed all three real loopback tests: two-listener text/binary/subprotocol/query/header/remote-close acceptance; shutdown closes both actual peers with 1001 and joins the tracker to zero active; unreachable upstream closes the accepted downstream with 1011 `proxy error`. The workspace registry is a fixture port in these transport tests, not evidence for cloud provisioning. Scoped all-target Clippy passed after the relay landed.

Additional repairs preserve ECMAScript PTY cursor radix/whitespace handling, literal/discriminated-union labels, optional named payload labels, and current integer NaN messages. Full-host wrong-method requests now use the actual UI wildcard fallback, not axum's 405. UI absence remains a typed 503; it is not changed to a fake baseline failure.

The next immutable empty-host replay measured **140/430 passing and 290 failing: 277 not-ported, 7 body, 2 status, 4 not-mounted**, before the final literal-union/NaN/wrong-method repairs. Residual fallback cases are unknown API/non-API paths and full-host `/openapi.json`, all requiring the real UI adapter. Current compact/wait recordings deliberately return baseline unavailable errors, while an empty host fails earlier at session placement. These are not grounds to bless the ledger. Full exact remaining IDs are available through the new opt-in `REPLAY_DIAGNOSTICS=1`; assertions and fixtures are unchanged.

A repeat complete scoped suite hit its 240-second wall-clock limit after 217 seconds waiting for the shared build lock. All six new HTTP tests and the executed preceding suites passed, but that timed run is not reported as a complete suite pass. Final unrestricted scoped rerun remains required.

Protocol inventory currently declares no non-JSON payload endpoint (`payload_encoding` has no call sites), so the earlier generic non-JSON implementation limitation is not a presently declared endpoint gap.

No production user data, TypeScript baseline source, manifests, crate edges, expected-failure ledger or fixtures were edited in this delivery.

## Follow-up: concrete remote workspace history and SSE

`runtime/src/workspace_transport.rs` now implements the real `SyncTransport` port with the root-supplied reqwest client. The supervisor already supplies routed endpoint URLs, so the transport never appends them a second time. Requests preserve adapter headers, set SSE Accept or JSON Content-Type, retain exact non-2xx SyncHttp messages and history error bodies, and return a response-owned byte stream. No detached transport task survives dropping that stream.

At **11:55 UTC**, all three `runtime/tests/workspace_transport.rs` tests passed. The production Process supervisor test uses actual SQLite/event storage and two native loopback listeners: initial history sequence 0, live sequence 1, reconnect with persisted fence 1, live sequence 2, recorded remote workspace ownership, joined shutdown, and observed remote stream cancellation. The remote adapter only supplies a controlled target; cloud provisioning is not claimed. Separate real-listener tests prove headers, invalid response bodies, typed HTTP errors, and standalone stream-drop cancellation.

At **11:57 UTC**, the unrestricted server/protocol package suite passed with only the separately measured immutable replay ledger assertion excluded. This includes six domain HTTP tests, eight SSE tests, three remote relay tests, and seven PTY socket tests. Server/protocol **all-target Clippy with `-D warnings` also passed**. A registry regression additionally proves `session.prompt_async` advertises only empty 204, not the constructor's earlier accidental JSON 200 alternative.

Strict Clippy for the runtime workspace transport target was attempted and blocked by seven newly edited provider variant compilation errors (`serde_json::Map` cannot be constructed from a two-element array with `.into()`). This is a peer-owned dependency failure, reported to the provider owner, not a transport lint pass. Reverification is pending that repair.

## Production replay follow-through

The final empty-host diagnostic at **11:57 UTC** measured **142/430 passing, 288 failing: 277 not-ported, 5 body, 5 not-mounted, 1 status**. The immutable ledger assertion still fails and was not blessed. Explicit residual body mismatches are compact/wait placement failures; UI fallback cases require an actual UI adapter, and the TUI control timeout requires the real process queue.

The independent production Process differential then measured **347/430 passing, 83 failing**. This is the first composed-host observation, not interchangeable with the empty-host diagnostic. Actual failures drove these API repairs:

- Durable current session SSE no longer inherits the handcrafted global stream cache/buffering headers. The TS handler returns the HttpApiBuilder-encoded stream without those extra headers. Actual loopback SSE tests passed after adding absence assertions.
- Named object validation now uses the preserved Effect identifier, so `QuestionV2.Reply` is not shortened to the OpenAPI component key. Its new actual HTTP regression passed.
- Legacy success alternatives are canonicalized by status with the last explicitly supplied schema replacing the constructor's unknown 200. Registry tests passed, including the auth boolean schema and sole async-prompt 204.
- The full UI port now accepts an owned native request and returns an owned response, preserving method, URI, headers, body stream, status and response stream. Existing static `get` adapters remain compatible. Both actual loopback port-fixture tests passed; actual upstream/CSP behavior remains root's runtime adapter responsibility.
- Empty current session lists now preserve the independently recorded `cursor.previous: null` and `cursor.next: null`; message paging remains separately governed by its own recorded behavior.
- Current filesystem raw bytes have an additive `handle_bytes` port with explicit status/MIME headers, avoiding JSON or UTF-8 corruption. A non-UTF-8 loopback regression is queued; root's global adapter owner supplies actual file reads.
- Current integration lifecycle routes now select the concrete provider credential port rather than the MCP/reference integration port.

The document endpoint was traced to a literal OpenAPI stub. Root delegated the real registry/IR emitter and its tests to a separate worker; this owner does not edit those delegated files. Final combined checks are temporarily blocked while that new module set is being written. Later runtime target Clippy reached the host and reported four peer-owned lints, routed to their owners; no runtime lint success is claimed yet.
