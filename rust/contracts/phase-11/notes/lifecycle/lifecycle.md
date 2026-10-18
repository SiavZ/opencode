# p11.lifecycle.lifecycle notes (readiness, single instance, quit/relaunch, onboarding, env config)

Owner: p11.lifecycle.lifecycle. Files: `crates/desktop-host/src/lifecycle.rs`, `src/lifecycle/*.rs`,
`tests/lifecycle_*.rs`. No dependency on other desktop-host modules (connections/platform are separate nodes).

## Contract surface (crate `opencode-desktop-host`, module `lifecycle`)

| Rust item | Baseline | Notes |
| --- | --- | --- |
| `readiness::Readiness<T>::{new, succeed, fail, wait, try_get, is_complete, forward_failure}` | `index.ts` `serverReady` Deferred, `initialization.ts` `forwardInitializationFailure` | One-shot watch cell. First completion wins. Errors, panics and a dropped loading future fail every waiter (`Failed`/`Panicked`/`Cancelled`). Late waiters get the stored outcome. |
| `readiness::ServerReadyData` | `preload/types.ts:19-23` | `username`/`password` serialize as `null`, never omitted. Duplicates `connections::ServerReadyData` (same wire shape), see open questions. |
| `single_instance::{acquire, Arbitration, PrimaryInstance::{recv, try_recv, shutdown}, SecondInstance}` | `index.ts:198-216` `requestSingleInstanceLock` / `second-instance` | Lock file `instance.lock` (create_new, PID inside) + `instance.sock` (unix) or named pipe `pipe_name` (windows, cfg). Secondary sends one JSON line `{argv, cwd}` and waits for an `ok`/`busy` ack, max 64 KiB, 5 s I/O timeout. Stale lock: 5 retries of 40 ms then reclaim. Bounded queue (16): full gives `Rejected`. |
| `quit::QuitCoordinator::{register, request, relaunch, cancel, is_quitting, subscribe_quitting}` | `index.ts:167-177,224-251`, `windows.ts:48-74` | Phases `BeforeQuit` then `WillQuit`, hooks inside a phase run concurrently and are joined. Each hook gets a `CancellationToken` and a timeout. `BestEffort` failures are reported but exit continues (baseline `stopSidecars().finally(app.quit)`). `Required` failure gives `Aborted` and restores `Running`. `cancel()` or a dropped driver gives `Cancelled` and restores `Running`. Concurrent requests join. Relaunch is sticky for the attempt. The shell does the native exit/relaunch after `Exit`. |
| `onboarding::{SettingsStore, has_existing_app_state, read_entries, initialize_old_layout_eligibility, is_old_layout_eligible, is_first_launch_onboarding_pending, finish_first_launch_onboarding, default_server_url, set_default_server_url}` | `onboarding.ts`, `install-state.ts`, `server.ts:30-42`, `store-keys.ts` | Works over a `SettingsStore` trait. The real conf-compatible store adapter belongs to p11.persistence (`store.rs`). |
| `config::{Channel, app_identity, test_onboarding, SidecarVersion, create_onboarding_test_root, base_env_overrides, ensure_loopback_no_proxy, proxy_from_env, ProxyEnv::proxy_for, enable_features_switch, command_line_switches, sidecar_port_from_env, local_sidecar_url, updater_enabled}` | `constants.ts`, `index.ts:53-140,191-196,350-375`, Node 24 `lib/internal/http.js` | Pure functions over `EnvMap` (BTreeMap). They never read or write the real process env. |

## Inventory (baseline -> Rust -> test, all fixture/in-process)

| Baseline | Rust | Test |
| --- | --- | --- |
| `index.test.ts` "forwards loading task failures before renderer initialization" | `Readiness::forward_failure` | `lifecycle_readiness.rs::forwards_loading_task_failures_before_renderer_initialization` |
| `index.test.ts` "... while renderer initialization waits" | same | `lifecycle_readiness.rs::forwards_loading_task_failures_while_renderer_initialization_waits` |
| `Effect.die` is forwarded as a defect | panic capture | `defects_are_forwarded_like_effect_die` |
| `serverReady` waiters in `awaitInitialization` | `Readiness::wait` | `all_waiters_resolve_on_success_and_late_waiters_see_stored_value`, `failure_after_success_does_not_override_readiness`, `dropping_the_loading_task_fails_waiters_instead_of_hanging`, `server_ready_data_wire_shape_keeps_nulls` |
| `index.ts:198-216` single instance | `single_instance::acquire` | `lifecycle_single_instance.rs` (primary + secondary in one process, stale lock and socket, full queue, oversized argv, garbage client, drop frees lock) |
| `index.ts:224-251` before-quit/will-quit, `stopSidecars().finally(quit)` | `QuitCoordinator::request` | `lifecycle_quit.rs` (6 tests, paused tokio clock) |
| `index.ts:167-177` relaunch | `QuitCoordinator::relaunch` | `concurrent_requests_join_and_relaunch_is_sticky` |
| `onboarding.ts`, `install-state.ts` | `onboarding::*` | `lifecycle_onboarding.rs` (7 tests) |
| `constants.ts`, `index.ts:53-69` channel/app names/ids, TEST_ONBOARDING, SIDECAR_VERSION | `config::{Channel, app_identity, test_onboarding, SidecarVersion}` | `lifecycle_config.rs::{app_names_and_ids_match_baseline, channel_parse_falls_back_to_dev, env_flags_require_exact_one, identity_uses_dev_when_unpackaged_and_onboarding_root_when_testing, onboarding_root_creates_isolated_dirs}` |
| `index.ts:95-113` `ensureLoopbackNoProxy` | `config::ensure_loopback_no_proxy` | `loopback_no_proxy_is_appended_to_both_keys` |
| `index.ts:72-79` `useEnvProxy` -> Node `setGlobalProxyFromEnv` | `config::{proxy_from_env, ProxyEnv::proxy_for}` | `proxy_env_resolution_and_bypass`, `no_proxy_matching_follows_node_proxy_config` |
| `index.ts:191-196` command-line switches | `config::command_line_switches` | `command_line_switches_match_baseline_order` |
| `index.ts:350-375` sidecar port env | `config::sidecar_port_from_env` | `sidecar_port_uses_parse_int_semantics` |

## Proxy semantics (fixed while resuming)

Node 24 `parseProxyUrl` does `env.https_proxy || env.HTTPS_PROXY`, so lowercase wins and empty strings fall through.
It honors only `http://`/`https://` proxy URLs, and reads `env.no_proxy || env.NO_PROXY`. `ensureLoopbackNoProxy`
always writes a non-empty lowercase `no_proxy`, so in the baseline a user list set only in uppercase `NO_PROXY` is
shadowed by the loopback-only lowercase list. The port keeps that quirk and the test asserts it. Matching follows
`ProxyConfig.shouldUseProxy`: `*`, exact host, exact `host:port`, `.suffix` (also bare suffix), `*.suffix`, and IPv4
`a-b` ranges. A bare `example.com` does NOT match subdomains. The earlier draft matched subdomains and ports
differently and read uppercase first for no_proxy semantics, which is why the test failed.

## Fixture vs live

All lifecycle tests are in-process fixture tests. Single instance uses real unix sockets and lock files in a tempdir
(live OS IPC, no user data). Nothing touches the real process env or the user data dir.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Windows named pipe transport | compiled and tested only on unix | no windows target or CI host here | p11 / integration | `cargo test -p opencode-desktop-host --test lifecycle_single_instance` on windows |
| Shell wiring | registering quit hooks (sidecar stop, draft flush, updater timer), feeding `SecondInstance` to deeplinks/window focus, calling native relaunch/exit | needs the desktop shell bin | p11 shell / bin owner | end-to-end desktop launch test |
| Electron single-instance interop | a Rust primary and an Electron primary do not see each other | different lock mechanisms | migration owner | n/a, accepted: only one app version runs at a time |
