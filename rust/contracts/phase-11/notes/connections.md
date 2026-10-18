# p11.connections notes (server connections + platform adapters)

Owner: p11.connections. Commits: `e78c5c488` (adopted modules from the interrupted worker + clippy
fixes), `0b991102c` (connection + live sidecar tests), `fa816fba6` (platform tests, proxy env fix),
`a083f0a25` (WSL tests), `31b23d459` (split under 500 lines), `ee59a8ed3` (headers).

## Contract surface (crate `opencode-desktop-host`)

| Rust item | Baseline | Notes |
| --- | --- | --- |
| `connections::ServerConnection::{OwnedSidecar, Daemon, Remote, Wsl}` | `index.ts:67,333-404`, `app/src/context/server.tsx:190-243` | `owned_by_host()` is true **only** for `OwnedSidecar`. `key()` is `sidecar` / `wsl:<distro>` / remote URL. `basic_auth()` uses `opencode` as the default username. |
| `connections::ServerReadyData {url, username, password}` | `preload/types.ts:19-23` | Wire JSON matches the TS shape, nullable fields serialize as `null`. |
| `connections::SidecarSlot::{set, current, kill_sidecar}` | `index.ts:88-93` (`killSidecar`) | The slot is cleared before awaiting. Only owned children get a joined process-tree stop. Daemon/remote/WSL are just dropped from the slot. |
| `connections::ConnectionError` | `server.ts:96-162` errors | `Spawn`, `Port`, `ReadyTimeout`, `ExitedBeforeHealthy`, `HealthTimeout`, `Command`, `Io`, `Unsupported`, `Cancelled`. |
| `connections::sidecar::{spawn_local_sidecar, SidecarConfig, serve_args, sidecar_env, port_from_env, allocate_port, new_password}` | `index.ts:333-404`, `server.ts:57-184,211-218`, `sidecar.ts:51-101` | The shell supplies the program (Rust `opencode` binary) and `serve_args(host, port)` (`serve --hostname --port --cors oc://renderer`). The Electron utilityProcess + in-process `Server.listen` becomes a child process. |
| `connections::child::{ChildServer, ChildSpec, HealthFailure}` | `server.ts:57-184`, `wsl/sidecar.ts:17-134` | Own process group. Recent output ring of 12 lines. `wait_healthy` polls every 100ms and races exit/timeout/cancel. `stop()` sends SIGTERM to the group, waits for grace, then SIGKILL, reaps, sweeps the group and joins the readers. `kill()` does the same with no grace. Dropping without stop kills the tree. |
| `connections::health::{check_health, BasicAuth}` | `server.ts:186-210` | `/api/health` then `/global/health`, 3s per request, basic auth. |
| `connections::background_cli::{start_background_cli, stage_cli, run_cli, state_home_candidates, service_url, PasswordCommand}` | `background-cli.ts:19-125` | Returns `ServerConnection::Daemon` (never killed). See blocker B1 for `PasswordCommand`. |
| `platform::process_tree::{isolate, signal_tree, terminate_tree, kill_tree, ExitInfo}` | Node `child.kill` / `taskkill` | unix: `process_group(0)` + `kill -SIG -- -pgid`. Windows: `CREATE_NEW_PROCESS_GROUP` + `taskkill /T [/F]` (blocked: no runner). |
| `platform::proxy::{ensure_loopback_no_proxy, upsert_loopback, EnvProxy, http_client, process_env}` | `index.ts:95-113`, `sidecar.ts:91-128` | Lowercase proxy var wins unless it is empty (JS `\|\|`). Auto system-proxy detection is off, so env is the only source. |
| `platform::shell_env::{load_shell_env, probe, parse_shell_env, merge_shell_env, resolve_user_shell, user_shell, is_nushell, prefer_app_env}` | `shell-env.ts:1-101`, `server.ts:44-55` | Tries `-il` first. A timeout gives up with no `-l` fallback. Nushell is skipped. |
| `platform::apps::{check_app_exists, resolve_app_path, resolve_windows_candidates, open_path, Os}` | `apps.ts:13-136`, `ipc.ts:81-82,219-226` | `Os` is a parameter, so non-native branches can be tested anywhere. |
| `platform::wsl::controller::WslServersController` (+ `controller/jobs.rs`) | `wsl/servers.ts:62-436` | Per-server generation counter. Stale starts stop their sidecar and never publish. Jobs are exclusive (a new job cancels the old one). Background tasks are tracked and joined by `shutdown(grace)`. `stop_all` bumps every generation, then stops and joins owned WSL sidecars. |
| `platform::wsl::runtime::{WslRunner, probe_runtime, list_installed, list_online, install_*, probe_distro, resolve_opencode, read_command_version, open_terminal, parse_*}` | `wsl/runtime.ts:9-405` | `WslRunner` trait. `exe::ExeRunner` is the real adapter (timeout + cancel kill and reap, UTF-16LE decode). |
| `platform::wsl::sidecar::{spawn_wsl_sidecar, bootstrap_script, RunningSidecar, SidecarHandle, SpawnSidecar}` | `wsl/sidecar.ts:17-134` | `bash -se` stdin bootstrap, 30s health timeout, typed failure messages. |
| `platform::wsl::policy::*` | `wsl/policy.ts`, `wsl/startup.ts`, `renderer/wsl/connections.ts`, `wsl/ipc.ts:70-87`, `wsl/servers.ts:453-506` | Pure functions plus `poll_health`. |
| `platform::wsl::{WslServersState, WslServersEvent, WslJob, Record, messages}` | `app/src/wsl/types.ts`, `app/src/i18n/desktop-native.ts:298-318` | camelCase / kebab-case wire tags. `Record` keeps insertion order. |

## Inventory (baseline -> Rust -> test)

| Baseline | Rust | Test |
| --- | --- | --- |
| `index.ts:88-93` killSidecar | `SidecarSlot::kill_sidecar` | `connections.rs::kill_sidecar_clears_but_never_stops_non_owned_connections`, **live** `connections_live.rs::live_python_sidecar_is_healthy_then_kill_sidecar_leaves_no_survivor` |
| `index.ts:95-113` ensureLoopbackNoProxy | `proxy::ensure_loopback_no_proxy` | `platform.rs::loopback_hosts_are_appended_once_case_insensitively` |
| `index.ts:333-404` local sidecar start (port, password, env override) | `sidecar::{port_from_env, allocate_port, new_password, spawn_local_sidecar}` | `connections.rs::{opencode_port_follows_parse_int_semantics, allocates_a_free_loopback_port, serve_args_and_password_shape}`, live tests |
| `server.ts:44-55` preferAppEnv | `shell_env::prefer_app_env` | `platform.rs::prefer_app_env_sets_desktop_flags_and_state_home` |
| `server.ts:57-184` spawnLocalServer (health wait, exit-before-healthy, timeout) | `ChildServer::{spawn, wait_healthy, stop, kill}`, `spawn_local_sidecar` | **live** `connections_live.rs::{live_child_exit_before_healthy_propagates_typed_error, live_ready_timeout_kills_the_child, live_spawn_failure_and_cancellation_are_typed, live_wait_healthy_reports_exit_with_recent_output, live_child_records_recent_output_and_sigterm_resistant_tree_is_force_killed}` |
| `server.ts:186-210` checkHealth | `health::check_health` | `connections.rs::health_falls_back_to_global_health_with_basic_auth` (real axum server) |
| `server.ts:211-218`, `sidecar.ts:75-101` sidecar env | `sidecar::sidecar_env` | `connections.rs::sidecar_env_sets_auth_state_home_and_loopback_no_proxy` |
| `sidecar.ts:111-120` useSystemCertificates | `proxy::http_client` (rustls platform verifier via reqwest `rustls`) | Not separately tested (see what was not checked) |
| `sidecar.ts:122-128` useEnvProxy | `proxy::{EnvProxy, http_client}` | `platform.rs::{env_proxy_prefers_lowercase_and_ignores_empty, http_client_routes_through_env_proxy_but_not_for_no_proxy_hosts}` |
| `background-cli.ts:19-58` startBackgroundCli | `background_cli::start_background_cli` | `connections.rs::background_cli_stages_discovers_existing_daemon_and_reads_password` (real scripted CLI process) |
| `background-cli.ts:60-78` installCli | `background_cli::stage_cli` | `connections.rs::stage_cli_copies_once_and_reuses` |
| `background-cli.ts:80-125` run/serviceUrl/executableName | `run_cli`, `service_url`, `executable_name`, `version_dir` | `connections.rs::background_cli_pure_helpers` |
| `background-cli.ts:27-29` candidates | `state_home_candidates` | `connections.rs::state_home_candidates_dedupe_and_keep_only_existing_dirs` |
| `shell-env.ts:12-101` (+ `shell-env.test.ts` 5 tests) | `shell_env::*` | `platform.rs` ports all 5, plus **live** `live_login_shell_probe_captures_env` and `shell_probe_falls_back_to_l_and_gives_up_on_timeout` |
| `apps.ts:13-37` checkAppExists/macOS | `apps::check_app_exists` | `platform.rs::app_detection_per_os` |
| `apps.ts:39-136` resolveWindowsAppPath | `apps::resolve_windows_candidates` (+ `where` in `resolve_app_path`) | `platform.rs::windows_candidate_resolution_follows_cmd_shims` (real shim files on macOS). `where` itself is blocked. |
| `ipc.ts:219-226` open-path | `apps::open_path` | `platform.rs::open_path_failures_are_typed` (the default-handler branch is not exercised so no real app opens) |
| `wsl/servers.test.ts` (13 tests) | controller + policy | 9 policy tests in `wsl_policy.rs` + 4 controller tests in `wsl_servers.rs`, ported 1:1 with a fake `WslRunner` |
| `wsl/servers.ts:62-436` generation counters, stop on remove, exit watch, jobs, install restart | `WslServersController` | `wsl_servers.rs::{stale_start_generation_is_disposed_and_never_published, sidecar_exit_after_startup_marks_the_server_failed, failed_start_publishes_message_and_new_job_cancels_previous, install_opencode_verifies_version_and_restarts_server, shutdown_cancels_pending_spawns_and_joins_tasks}` |
| `wsl/servers.ts:438-506` persisted normalization, opencodeCheck | `policy::{normalize_persisted_servers, opencode_check}` | `wsl_policy.rs::normalizes_persisted_servers`, `wsl_runtime.rs::opencode_check_shapes` |
| `renderer/wsl/connections.test.ts` (3 tests) | `policy::{ready_wsl_connections, available_startup_server}` | `wsl_policy.rs::{publishes_a_wsl_server_only_after_it_reports_ready, does_not_block_desktop_startup_on_a_configured_wsl_default}`. The translated label test belongs to the renderer and is not a host concern. |
| `app/src/wsl/types.ts` state wire | `WslServersState` serde | `wsl_policy.rs::wire_shape_matches_typescript_state` |
| `wsl/runtime.ts:37-405` args, parsing, decoding, run | `runtime::*`, `exe::{ExeRunner, decode_output}` | `wsl_runtime.rs` (parsers, UTF-16LE, escape, ExeRunner exit/timeout/cancel with real POSIX processes) |
| `wsl/sidecar.ts:17-134` | `sidecar::{spawn_wsl_sidecar, bootstrap_script}` | `wsl_runtime.rs::bootstrap_script_matches_wsl_sidecar_ts`. The spawn path shares `ChildServer`, which is covered live with `bash -s` stdin (`live_stdin_script_is_delivered_like_wsl_bash_se`). |
| `wsl/ipc.ts:70-87` unavailable handlers | `policy::unavailable_state` | `wsl_policy.rs::wire_shape_matches_typescript_state` |

Test counts (macOS arm64, `CARGO_TARGET_DIR=target/p11`): connections 12, connections_live 7, platform 14,
wsl_policy 12, wsl_servers 9, wsl_runtime 8. All pass. `cargo clippy -p opencode-desktop-host --tests -- -D warnings` is clean.

## Fixture vs live

- Live (real processes): `connections_live.rs` (real `python3 -m http.server` under `sh`, process-group
  liveness via `kill -0 -pgid`), background CLI tests (real executable script), shell probes
  (`/bin/sh` plus temp scripts), `ExeRunner` POSIX tests, axum health/proxy servers.
- Fixture: `wsl_servers.rs` (fake `WslRunner` + fake `SidecarHandle` in `tests/wsl_support/mod.rs`), pure policy and parser tests.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| B1 background CLI password | Desktop baseline runs `service get password` (`background-cli.ts:43`), but the CLI spec (`packages/cli/src/commands/commands.ts:36-39`) and the Rust CLI (`bins/opencode/src/cli/spec/v2.rs:77`) only declare `service password`. Against a spec CLI, the baseline vocabulary fails. | Baseline inconsistency. Rust keeps `PasswordCommand::DesktopBaseline` as the default for parity and offers `CliSpec`. | p11 root + CLI owner | Decide the vocabulary. Evidence: `connections.rs::password_vocabulary_discrepancy_is_typed` |
| B2 Windows process tree | `taskkill /T [/F]`, `CREATE_NEW_PROCESS_GROUP`, `CREATE_NO_WINDOW` paths | No Windows runner | p11 / CI | Windows CI running `connections_live.rs` equivalents |
| B3 WSL real adapter | `ExeRunner` against real `wsl.exe` (UTF-16 output, `--list`, distro probes, `bash -se` sidecar) | No Windows+WSL runner. `wsl_runtime.rs::real_wsl_exe_probe_runtime` is `cfg(windows)` + `#[ignore]` | p11 / CI | Windows+WSL runner |
| B4 WSL interactive installs | `run_interactive` uses piped stdio, not ConPTY (`runtime.ts:116-178` uses node-pty) | No pty dependency and no runner | p11 root (dep decision) | ConPTY-backed adapter + Windows test |
| B5 Windows `resolveAppPath` `where` and `explorer` open | `where` invocation and the default `explorer` open | No runner. Shim resolution is tested with real files on macOS. | p11 / CI | Windows runner |
| B6 Linux | `LD_PRELOAD` strip and `xdg-open` | No Linux runner. The env strip is unit-tested via the `linux` flag. | p11 / CI | Linux CI run of these tests |
| B7 system CA parity | Node merges default + system CAs. Rust relies on reqwest's `rustls` feature trust roots. Whether that includes the OS store depends on the workspace reqwest feature set. | Not verified with a private CA | p11 root | Test with a custom CA in the OS store |
| B8 sidecar program | Baseline runs the server in-process (utilityProcess `sidecar.js` -> `Server.listen`). Rust spawns a child binary with `serve_args`. Live proof uses python, not the Rust opencode server. | The Rust server binary readiness is owned elsewhere | p11.shell | Shell integration test launching `opencode serve` with temp HOME/XDG |
| B9 localization | `messages` are the English strings from `desktop-native.ts` | The native translation layer is owned elsewhere | p11 root | Wire through the i18n layer |
