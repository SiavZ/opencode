# Phase 14 leaf: Slack Socket Mode

Date: 2026-10-03. Node: `p14.slack`. Library implemented and checked with loopback fixtures. This is not real-workspace qualification or hosted-bin delivery.

## Public boundary

`opencode_ecosystem::slack` exports `api`, `bot`, `output`, `socket`, `thread` and typed `SlackError`. Compose `SlackApi::new(api_base, app_token, bot_token)`, an existing `Arc<dyn SessionPort>`, and `SlackBot::new(api, sessions, BotConfig::default())`. Await `bot.run(cancellation_token)` and cancel then await it during shutdown. The caller owns its actual session host and any child process. Production uses tokio-tungstenite and reqwest, not Bolt or retained first-party TypeScript routing.

API base URLs and Socket Mode URLs are injected through `SlackApi` and `apps.connections.open`. `auth.test` resolves the bot user for Bolt-compatible self filtering. App and bot tokens have separate authorization headers. Credentials are not included in the API client's Debug output. No dependency or first-party crate edge was added.

## Baseline inventory

| Item | Baseline | Implementation | Fixture proof |
| --- | --- | --- | --- |
| Socket Mode URL / credentials | `packages/slack/src/index.ts:4-9`; pinned SocketModeClient `retrieveWSSURL:331-345` | `src/slack/api.rs`, `src/slack/socket.rs` | `fixture_full_bot_ack_dedup_test_command_tools_and_reconnect`; fixture API verifies app vs bot authorization |
| Message receipt ACK | Pinned SocketModeClient `send:300-329`, `onWebSocketMessage:588-648`; Bolt App event ACK around `553-559` | Empty-payload `{envelope_id,payload:{}}` before dispatch, independent of session latency | Full bot and blocked-prompt fixtures assert ACK before response |
| Hello / disconnect / close | Pinned SocketModeClient `connected:176-219`, message handling `588-648` | Hello marks readiness; disconnect warning, close, transport loss and liveness timeout reconnect; cancellation interrupts backoff | Full bot, socket-close and heartbeat fixtures |
| Retry fields / redelivery | Pinned SocketModeClient `onWebSocketMessage:588-648` exposes retry attempt/reason | Envelope retains `retry_attempt` and `retry_reason`; bounded event-ID or envelope-ID dedup survives reconnect | `fixture_envelope_dedup_is_bounded_and_expires`, full bot retry fields, socket-close fixture |
| Subtype / no text / bot filtering | `packages/slack/src/index.ts:58-70`; Bolt `middleware/builtin:253-285` ignoreSelf | Valid message events only, no nonempty subtype, no empty text, skip authenticated bot user; thread_ts or ts | `fixture_message_filters_and_thread_selection`; full bot self/subtype envelopes still ACK |
| Thread creation and mapping | `packages/slack/src/index.ts:22,68-94` | Process-local channel/thread tuple map; per-key serialized creation and prompts; title `Slack thread <ts>` | `fixture_thread_serialization_and_channel_isolation` proves one first-session creation, same-key serialization and cross-key concurrency |
| Create failure | `packages/slack/src/index.ts:78-89` | Exact create failure reply, no retained session ID; later event can retry creation | `fixture_session_failure_fallbacks_and_reply_precedence` |
| Share URL and failure | `packages/slack/src/index.ts:93-101` | Map before share; share error ignored; URL posted to mapped thread; missing successful URL preserves omitted-text post and upstream error path | Failure/fallback fixture and `fixture_missing_share_url_preserves_omitted_text_post` |
| Prompt and final output | `packages/slack/src/index.ts:104-135` | `PromptRequest::text`; exact prompt failure reply; legacy info.content first, joined text parts second, exact empty-output fallback last | Failure/fallback fixture, full bot fixture |
| Tool completed updates | `packages/slack/src/index.ts:23-50` | Filter message.part.updated, type tool, status completed and session ID; post `*<tool>* - <title>` to mapped thread; ignore post failure | `fixture_tool_updates_filter_session_and_completed_state`, full bot injected SessionEvent |
| Slash `/test` | `packages/slack/src/index.ts:138-142` | Immediate envelope ACK then exact bot-working reply to command channel | Full bot fixture |
| HTTP failures / limits | Installed Slack Web API adapter behavior behind Bolt | Typed API code, HTTP status, transport, rate-limit and protocol errors; bounded retry honoring short Retry-After | `fixture_slack_api_errors_and_rate_limit_retry` |
| Shutdown | Baseline startup `packages/slack/src/index.ts:144-145`; Rust task ownership rule | Child CancellationToken, locally owned socket JoinHandle and bounded JoinSet, cancelled and joined on awaited exit | `fixture_shutdown_joins_blocked_prompt_and_ack_is_immediate`; heartbeat/backoff cancellation fixture |

Pinned dependency source paths inspected locally:

- `node_modules/.bun/@slack+socket-mode@1.3.6+302f8eed98f6f908/node_modules/@slack/socket-mode/dist/SocketModeClient.js`.
- `node_modules/.bun/@slack+bolt@3.22.0+302f8eed98f6f908/node_modules/@slack/bolt/dist/App.js` and `receivers/SocketModeReceiver.js:123-145`.
- The same Bolt package's `middleware/builtin.js:253-285` for ignoreSelf.

## Decisions and bounded behavior

**Approved privacy correction:** Do not port baseline raw event/context/message/prompt/result logs (`packages/slack/src/index.ts:53-59,66,104-110,132,140`). The assignment explicitly approves this correction. Rust logs failure markers only, not raw message bodies, prompts, provider responses, credentials, connection URLs or share URLs. Tool-post failures remain ignored as in the baseline, with a content-free marker.

The assignment requests per-key serialization and bounded dedup, so the unguarded baseline Map becomes a serialized process-local map. A tuple key also avoids delimiter collisions. Defaults: 4096 thread entries, 600-second entire thread operation timeout including gate wait, queue capacity 256, 32 workers, dedup capacity 4096 and TTL 300 seconds. `ThreadRouter::with_limits` and Socket/Bot configurations support smaller test limits. New thread overflow returns `ThreadLimit`. Queued-operation timeout returns `Timeout` and releases the gate.

A fresh envelope reserves queue capacity before ACK. Saturation returns `QueueFull` without ACK or silent drop; the host observes the typed failure and decides whether to restart the bot. Duplicate envelopes are ACKed even without free queue capacity. ACK means local receipt, not durable prompt admission, executed response or successful Slack post. Cancellation can discard ACKed but unexecuted work. This library does not claim durable delivery or exactly-once execution.

Default reconnect delay grows from 500 ms to 30 seconds. Each connection/ACK wait is bounded by the 30-second connection timeout. Websocket frame and assembled-message sizes are enforced inside tungstenite before oversized application payload allocation, default 1 MiB. Malformed JSON is ignored without logging its contents. Liveness uses configurable ping interval and timeout, default 5 seconds and 30 seconds. These bounded timings are not asserted to reproduce the pinned SDK's separate client/server heartbeat clocks exactly.

Web API calls have 30-second per-request timeouts and three retries by default, configurable up to ten. Short Retry-After delays are honored; delays above 30 seconds return `RateLimited` instead of an unbounded sleep. Network and server failures retry with exponential delays. This is a bounded Rust policy rather than a claim of identical SDK retry jitter or maximum attempts. Retrying an ambiguously completed upstream chat post may duplicate a message. Invalid API JSON and `ok:false` are typed failures.

All tasks spawned by SlackBot are joined before its awaited run returns. The subscription receives the owned cancellation token and its receiver is dropped on shutdown. The existing SessionPort trait exposes a receiver, not the producer's JoinHandle, so producer task joining remains the session-port owner's responsibility. Cancelling an in-flight prompt future is not proof that remote model execution stops. Abandoning the run future triggers child-token cancellation but cannot itself await joins: the host must cancel and await the run.

## Validation

Executed from `rust/`, using `RUSTC_WRAPPER=sccache`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=3`, `SCCACHE_BASEDIRS=$PWD/target` and `CARGO_TARGET_DIR=$PWD/target/p14`:

- `cargo test -p opencode-ecosystem`: passed, including 15 Slack fixtures (8 bot/thread/output and 7 socket/API fixtures).
- `cargo clippy -p opencode-ecosystem --all-targets -- -D warnings`: passed.
- `cargo fmt -p opencode-ecosystem` followed by `cargo fmt -p opencode-ecosystem -- --check`: passed.
- Seven socket/API fixtures repeated five times after fixing fixture synchronization: all passed. The reconnect fixture now waits for the server to observe the first ACK before forcing a close, instead of racing its own server-side ACK recorder.
- Every Slack production/test `.rs` file has the mandatory header and fewer than 400 physical lines.

Fixtures use local axum Web API and websocket servers on 127.0.0.1, synthetic app/bot tokens, and a test SessionPort implementation. No real Slack credentials, remote network, cloud writes or real user data were used. Helper `.rs` files appear as zero-test integration targets as well as imported fixture modules.

## Blockers / proof still needed

| Item | Missing behavior or proof | Why | Owner | Proof needed |
| --- | --- | --- | --- | --- |
| Hosted Slack composition | Actual Rust Slack executable startup/configuration and approved artifact deployment | Leaf owns a library; baseline createOpencode child and process environment composition are outside these paths | Phase 13/14 parent, Phase 15 packaging | Owned host starts real SessionPort and SlackBot, cancels and awaits both, with Rust artifact and configuration contract |
| Session subscription producer ownership | Joining a producer task through SessionPort | Current subscribe contract returns only receiver and accepts cancellation token; SlackBot cannot join a hidden HTTP adapter task | Phase 14 parent / SessionPort owner | Owned subscription guard or joinable lifecycle contract and producer cancellation test |
| Real Slack acceptance | Real apps.connections.open, hello/disconnect, API authorization, rate limits, bot filtering and /test delivery | Only loopback fixtures are authorized; no staging workspace credentials were supplied | Phase 14 host reviewer / Phase 91 | Approved isolated Slack workspace with real adapter recordings and scoped credentials |
| Real session/share acceptance | Completed prompt semantics, EventV2 tool updates, approved share policy and remote prompt cancellation | Fake SessionPort verifies consumer behavior, not upstream execution or privacy policy | Phases 04/06/08/13 and Phase 91 | Real producer-to-Slack consumer run, share authorization proof and cancellation semantics |
| Retry/liveness qualification | Exact pinned SDK retry jitter, separate heartbeat clocks, token rotation and proxy/TLS failure behavior | Rust deliberately uses bounded configurable policy; not an exhaustive Slack SDK replacement | Phase 14 security/host reviewer | Approve bounded-policy differences and run transport recordings, expired-token and proxy/TLS harnesses |

Not checked: real Slack or hosted-bin operation, OS signal wiring, platform packaging, external proxy/TLS behavior, process restart recovery (not promised by the baseline), durable admission after ACK, or remote execution cancellation. Tool post errors are intentionally not retried beyond the Web API client's bounded retry budget.
