# Phase 15 baselines: TypeScript product on identified hardware

Status: measured, provisional. These are the baseline values decision D12 refers to. Budgets from
`docs/plans/rustport/production-gates.md` ("Proposed measurable budgets") apply against them:
Rust p95 at most +5% of baseline p95, peak RSS at most +5%. Absolute ceilings are still a Phase 92
blocking-proof item, not set here.

## Environment

| Field | Value |
| --- | --- |
| Date (UTC) | 2026-10-03T11:41:50Z |
| Machine | Apple M5 Max, 18 cores (`hw.ncpu`), 48 GiB (`hw.memsize` 51539607552) |
| OS / arch | macOS 26.6.2 (25G83), arm64 (`aarch64-apple-darwin`) |
| Runtime | bun 1.3.14 |
| Baseline source | branch `rust-port` HEAD `0164242afc80d63a68ddcb2848e98ca115736b42`, `packages/opencode` tree `212e389ea86ec333412dccb199145f7542b5c557` (last commit touching `packages/opencode`/`packages/core`: `a79ecfe10`) |
| Entrypoint | dev entrypoint `bun run src/index.ts` from `packages/opencode` (no compiled artifact, version reports `local`) |
| Isolation | every sample: fresh `mktemp` HOME + `XDG_{DATA,CONFIG,CACHE,STATE}_HOME`; `OPENCODE_*` env scrubbed; `serve --pure` (no external plugins); no provider credentials |
| Load | `vm.loadavg` 21.4 / 20.4 / 20.0 at start. The machine is shared by ~30 concurrent agents running cargo/bun builds |
| N | 12 samples per metric (240 latency samples for SSE) |

## Results

Times in milliseconds. Interpolated percentiles over the raw samples.

| Metric | p50 | p95 | p99 | min | max | stdev |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `--version` cold (fresh HOME, empty bun cache) | 1164 | 3762 | 4307 | 954 | 4443 | 1062 |
| `--version` warm (primed HOME) | 861 | 3222 | 3459 | 649 | 3518 | 977 |
| `--help` cold | 867 | 4665 | 5133 | 746 | 5250 | 1550 |
| `--help` warm | 1149 | 4554 | 5110 | 624 | 5249 | 1478 |
| `serve` spawn to first `GET /global/health` 200, cold HOME | 2890 | 5240 | 6081 | 1584 | 6292 | 1288 |
| `serve` spawn to first health 200, warm HOME | 2744 | 6617 | 7019 | 975 | 7119 | 2044 |
| SSE: `POST /session` start to matching `session.created` on `/event?directory=` | 6.4 | 56.8 | 457 | 1.4 | 1682 | 122 |

| Metric | p50 | p95 | min | max |
| --- | ---: | ---: | ---: | ---: |
| `serve` peak RSS, start + 1 health request (MiB) | 562 | 574 | 490 | 574 |
| `serve` peak RSS after workload: 120 sessions + SSE subscriber (MiB) | 538 | 549 | 520 | 550 |
| SSE burst: 100 sequential `POST /session`, `session.created` events/s | 237 | 396 | 38 | 461 |

All 12 bursts delivered every `session.created` event (no loss, `sse_burst_complete=true`).
RSS is `/usr/bin/time -l` "maximum resident set size" of the bun process (bytes / 2^20).

Single-run criterion cross-check (warm only, different harness, same machine, see below) is
recorded in the "Criterion" section.

## Noise notes

- Load average ~20 on 18 cores during the run. The p95/p99 tails (3 to 7 s startup, 1.7 s SSE
  max) are dominated by CPU contention from other agents, not by the product. The minima (650 ms
  CLI, 975 ms serve, 1.4 ms SSE) are the best estimate of uncontended cost.
- `--help` warm p50 > cold p50 is noise ordering (warm samples happened to coincide with a load
  spike), not a real effect. A 2-sample smoke run minutes earlier showed the opposite.
- Rust-vs-TS comparison under the 5% p95 rule MUST be run interleaved on the same machine in the
  same load window (alternate baseline/candidate samples), otherwise a quieter window will pass
  or fail the budget by itself. Phase 92 should rerun on an idle machine with `N>=30`.
- The dev entrypoint transpiles TypeScript on every start (bun cache under `XDG_CACHE_HOME`).
  A compiled `bun build --compile` artifact is the realistic release baseline. It was not built
  here (no packaging in this phase's scope); this is listed as a blocker below.
- Peak RSS "idle" > "after workload" because RSS is dominated by startup (module graph, ~560 MiB)
  and samples were in different load windows; the workload adds nothing measurable at this size.
- SSE throughput is bounded by the sequential HTTP client (one request in flight), so it is a
  request-latency-bound event rate, not a broker saturation figure.

## Reproduce

```sh
# from the repo root; never runs bun install, never touches the real HOME
BENCH_N=12 rust/benchmarks/scripts/baseline.sh "$JCODE_SCRATCH_DIR/ts-baseline.json"
# any other binary (e.g. the Rust candidate), same workloads:
BENCH_N=12 OPENCODE_BENCH_CMD="$PWD/rust/target/release/opencode" OPENCODE_BENCH_CWD=/tmp \
  rust/benchmarks/scripts/baseline.sh rust-candidate.json
```

`baseline.sh` records machine/toolchain metadata (sysctl, `bun --version`, git commit, loadavg) and
calls `baseline.py` (python3 stdlib only), which writes all raw percentiles plus per-burst runs as
JSON. Workloads:

1. CLI: `<cmd> --version` and `<cmd> --help`; cold = new temp HOME per sample, warm = one primed HOME.
2. Serve readiness: `<cmd> serve --port <free> --pure`, poll `GET /global/health` every 10 ms
   until 200, then SIGTERM.
3. RSS + events: under `/usr/bin/time -l`, start serve, (a) one health request, stop; (b) open
   `GET /event?directory=<git-init temp dir>`, wait for `server.connected`, 20x
   `POST /session?directory=...` measuring time to the matching `session.created` event, then a
   100-request burst and wait until all 100 `session.created` arrive.

## Criterion harness

`rust/benchmarks` (standalone cargo project, own `[workspace]`) holds process-level criterion
benches against `OPENCODE_BENCH_BIN` (+ optional `OPENCODE_BENCH_BIN_CWD`). It prints a `SKIP`
line per group and measures nothing when the variable is unset.

```sh
cd rust/benchmarks
OPENCODE_BENCH_BIN="bun run src/index.ts" OPENCODE_BENCH_BIN_CWD=$PWD/../../packages/opencode \
  cargo bench --bench process -- --save-baseline ts
OPENCODE_BENCH_BIN=$PWD/../target/release/opencode cargo bench --bench process -- --baseline ts
```

Groups: `cli_warm/version`, `cli_warm/help`, `serve/spawn_to_healthy_warm`,
`sse/session_create_to_event`.

Single run against the TS baseline, 2026-10-03T11:52Z, same machine and load window (criterion
`--save-baseline ts`, saved under `rust/target/p15/criterion/`, not committed). Values are
criterion's [lower, estimate, upper] of the mean:

| Bench | lower | estimate | upper |
| --- | ---: | ---: | ---: |
| `cli_warm/version` | 1.78 s | 2.43 s | 3.07 s |
| `cli_warm/help` | 928 ms | 1.53 s | 2.28 s |
| `serve/spawn_to_healthy_warm` | 2.77 s | 3.66 s | 4.51 s |
| `sse/session_create_to_event` | 5.40 ms | 6.59 ms | 7.78 ms |

Criterion warned it could not finish 10 samples in the 20 s/40 s budgets for `help` and `serve`
(contention). The SSE estimate agrees with the script's p50 (6.4 ms); startup means sit between
the script's p50 and p95 as expected under the same noisy load.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Rust candidate numbers | every metric above for the Rust binary | `bins/opencode` has no `serve`/HTTP yet and no release build exists | phase-09 / phase-91 | rerun `baseline.sh` and criterion `--baseline ts` interleaved with TS |
| Compiled TS baseline | same metrics for a `bun build --compile` release artifact | packaging not in this node's scope; dev entrypoint includes transpile cost | phase-15 (distribution) | build artifact in temp dir, rerun with `OPENCODE_BENCH_CMD=<artifact>` |
| Idle-machine run | low-noise p95/p99 | machine shared by ~30 agents (loadavg ~20) | phase-92 | N>=30 interleaved run at loadavg < 2 |
| Provider/tool overhead, large histories, UI/WASM, retained heap, 72 h soak | not measured | need provider fixtures/credentials, browser harness and long runtime | phase-15 follow-up / phase-92 | dedicated workloads |
| Linux arm64 baseline | same metrics in Docker `linux/arm64` | not run in this pass | phase-15 follow-up | run `baseline.sh` inside a container with bun |
