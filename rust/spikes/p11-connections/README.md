# Phase 11 connections spike

Date: 2026-10-15. Leaf: `p11.shell.connections-spike`. Commit: `49cfbca79`.

## Purpose

Exercise the desktop-host public local spawn/health/stop and remote connection APIs against
LOCAL temp fixtures. Prove process identity and cleanup. Capture commands and output.

## Scope and boundaries

- Owns ONLY `rust/spikes/p11-connections/**` (standalone `[workspace]`, excluded from main).
- Does NOT edit `rust/crates/desktop-host/` or any connections-owner files.
- No production data. All fixtures are tempdirs with bounded teardown.
- Actual packaged CLI (`opencode serve`) is unavailable in this environment (honest blocker B8).

## Inventory exercised

| API surface | Baseline | Rust | Test |
| --- | --- | --- | --- |
| `ChildServer::spawn` / `wait_healthy` / `stop` | `server.ts:57-184` | `connections/child.rs:122-294` | `tests/spawn_health_stop.rs::test_local_spawn_health_stop_with_temp_fixture` |
| `SidecarSlot::set` / `current` / `kill_sidecar` | `index.ts:88-93` | `connections.rs:130-165` | `tests/spawn_health_stop.rs::test_sidecar_slot_kill_sidecar_only_stops_owned` |
| `ServerConnection::Remote` not killed by slot | `index.ts:88-93` | `connections.rs:84-87` | `tests/spawn_health_stop.rs::test_remote_connection_not_killed_by_slot` |
| `check_health` basic auth fallback | `server.ts:186-210` | `connections/health.rs` | exercised indirectly via `wait_healthy` |

## Executed local spike

From `rust/spikes/p11-connections/`:

```sh
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export SCCACHE_BASEDIRS="$PWD/../../target" SCCACHE_CACHE_SIZE=100G
export CARGO_TARGET_DIR="$PWD/../../target/p11"
cargo test -- --nocapture
```

Captured result at 15:01 UTC:

```text
running 3 tests
test test_sidecar_slot_kill_sidecar_only_stops_owned ... ok
test test_remote_connection_not_killed_by_slot ... ok
test test_local_spawn_health_stop_with_temp_fixture ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
```

## Test details

### test_local_spawn_health_stop_with_temp_fixture

Spawns a real `python3` process serving `/api/health` and `/global/health` on a free loopback
port. Verifies:
- Child gets a PID
- `wait_healthy` succeeds within 5s timeout
- Identity: `child.url()` matches, `child.program()` contains "python3"
- `stop()` returns exit info
- Process is dead after stop (verified via `kill -0` on Unix)

### test_sidecar_slot_kill_sidecar_only_stops_owned

Creates an `OwnedSidecar` connection, sets it in the slot, calls `kill_sidecar()`. Verifies:
- Slot is cleared after kill
- `owned_by_host()` returns true for OwnedSidecar
- Process is dead after kill_sidecar (verified via `kill -0` on Unix)

### test_remote_connection_not_killed_by_slot

Creates a `Remote` connection pointing at a temp axum server, sets it in the slot, calls
`kill_sidecar()`. Verifies:
- Slot is cleared (Remote is dropped from slot)
- `owned_by_host()` returns false for Remote
- `key()` returns the URL for Remote
- Remote server is STILL healthy after kill_sidecar (proves it was not killed)

## Blockers and unchecked scope

| Item | Missing behavior | Why | Owner | Proof needed |
| --- | --- | --- | --- | --- |
| B8 real CLI | Tests use `python3` fixture, not `opencode serve` | Packaged CLI binary unavailable in spike env | p11.shell | Shell integration test launching real `opencode serve` with temp HOME/XDG |
| B2 Windows | Process tree kill uses unix `kill -0` check | No Windows runner | p11 / CI | Windows CI running equivalent tests |
| B3 WSL | WSL sidecar spawn not exercised | No Windows+WSL runner | p11 / CI | Windows+WSL runner |
| B7 system CA | TLS certificate handling not tested | No private CA fixture | p11 root | Test with custom CA in OS store |
| Background CLI | `start_background_cli` / `stage_cli` not exercised in this spike | Scope focused on spawn/health/stop | p11.connections | Separate spike or owner tests |
| Shell env | `load_shell_env` / `prefer_app_env` not exercised | Scope focused on spawn/health/stop | p11.connections | Owner tests in `platform.rs` |

## What was not checked

- Real `opencode serve` binary spawn and health (blocker B8)
- Windows process tree isolation and cleanup
- WSL sidecar bootstrap and health
- Background CLI staging and daemon connection
- Shell environment probe and merge
- Proxy env handling (covered by owner tests in `platform.rs`)
- System CA certificate loading
- Concurrent sidecar starts and generation counters (covered by owner WSL tests)
- Full crate clippy (owner code has unrelated lint errors)

## Commit

`49cfbca79` chore(spike): add p11-connections spike for spawn/health/stop testing

Files:
- `rust/spikes/p11-connections/Cargo.toml`
- `rust/spikes/p11-connections/Cargo.lock`
- `rust/spikes/p11-connections/src/lib.rs`
- `rust/spikes/p11-connections/src/fixtures.rs`
- `rust/spikes/p11-connections/tests/spawn_health_stop.rs`
