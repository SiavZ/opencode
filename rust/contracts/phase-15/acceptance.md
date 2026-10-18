# Phase 15 acceptance evidence: black-box workflows, TS baseline vs Rust candidate

Status: harness built and run against the TypeScript baseline on `aarch64-apple-darwin`. Every
Rust candidate row is **blocked: binary not built** (`rust/bins/opencode` has no `serve`/HTTP
surface and no release artifact yet). A baseline-recorded row is not a pass, it proves the
workflow expectations match the current product.

## Harness

`rust/tests/acceptance` (standalone cargo project, own `[workspace]`, reuses the process driver in
`rust/benchmarks/src`). It drives real processes only, never links product crates.

```sh
cd rust/tests/acceptance
cargo build --bin acceptance
BASELINE_CMD="bun run src/index.ts" BASELINE_CMD_CWD=$PWD/../../../packages/opencode \
CANDIDATE_CMD=$PWD/../../target/release/opencode \
  ../../target/<dir>/debug/acceptance --out evidence.jsonl   # --workflows DIR to override
```

- Workflow files: `rust/tests/acceptance/workflows/*.toml`, steps `cli`, `serve`, `http`,
  `sse_connect`, `sse_expect` with `expect_status`, `expect_json` (JSON pointer -> exact value),
  `expect_types`, `expect_array_item`, `capture` and `${var}` templating (`workdir`,
  `workdir_real`, captured values). Unknown fields are rejected.
- Each workflow run gets a fresh temp HOME/XDG, scrubbed `OPENCODE_*` env, and a fresh
  `git init` work directory. `serve` uses `--port <free> --pure`.
- Output: one JSON line per (workflow, role) with exactly the 13 production-gates.md evidence
  fields in order: `requirement_id, baseline_source, owner, producer_consumer, workflow,
  fixture_live, target, candidate_hash, command_run_id, expected, observed, reviewer,
  gate_decision`. `reviewer` is always `unreviewed` (an agent run is not review).
- `candidate_hash`: `<ROLE>_HASH` env override, else sha256 of an absolute executable, else
  `git-tree:<tree of the target cwd>` (the dev TS command runs from source). Resolved once per run.
- Gate decisions: `baseline-recorded`, `baseline-failed: ...`, `pass (pending review)`, `fail`,
  `blocked: <ROLE> binary not built or command unset`. Exit code 1 on any executed failure.
- Contract tests (`cargo test`, fixture): workflow parsing, exact field set/order, unset role is
  blocked, real `/bin/sh` stub passes and fails, JSON pointer/template behavior. 5/5 pass.

## Run record

| Field | Value |
| --- | --- |
| Date | 2026-10-03, run id `1791028266-27117` |
| Host / target | Apple M5 Max, macOS 26.6.2, `aarch64-macos` (D13 executable row) |
| Baseline | `bun run src/index.ts` from `packages/opencode`, bun 1.3.14, tree `212e389ea86ec333412dccb199145f7542b5c557` |
| Candidate | unset (Rust `bins/opencode` not built with serve/HTTP) |
| Classification | live (real processes, real HTTP/SSE on loopback, no provider credentials needed) |
| Result | 3/3 baseline-recorded, 3/3 candidate blocked, harness exit 0 |

| requirement | workflow | role | fixture/live | candidate hash | gate decision | observed (abridged) |
| --- | --- | --- | --- | --- | --- | --- |
| PKG-09 | cli-version | baseline | live | git-tree:212e389e | baseline-recorded | `--version` exit 0, stdout `local`, 1980 ms |
| PKG-04 | serve-health | baseline | live | git-tree:212e389e | baseline-recorded | healthy in 3416 ms; `GET /global/health` 200 `{"healthy":true,"version":"local"}`; `/global/event` first event `server.connected` |
| PKG-04 | session-create-list | baseline | live | git-tree:212e389e | baseline-recorded | healthy in 9328 ms; `/event?directory=` first `server.connected`; `POST /session` 200 (directory = realpath of workdir, cost 0, tokens 0); `session.created` with matching `sessionID` and `info.id`; `GET /session` lists it; `GET /session/{id}` 200 |
| PKG-09 | cli-version | candidate | live | none | blocked: binary not built | not run |
| PKG-04 | serve-health | candidate | live | none | blocked: binary not built | not run |
| PKG-04 | session-create-list | candidate | live | none | blocked: binary not built | not run |

Observations recorded while writing expectations (wire facts a Rust candidate must match):

- `POST /session?directory=<path>` returns `directory` as the canonical realpath (macOS
  `/var/...` becomes `/private/var/...`), `projectID` `"global"` for a fresh repo without commits.
- `/event?directory=` frames are `data: {"id","type","properties"}`; `/global/event` frames wrap
  the same shape in `{"payload": {...}}`. Both begin with `server.connected`.
- `--version` on the dev entrypoint prints `local` (`packages/core/src/installation/version.ts:6`).

## Raw evidence (JSON lines, home directory redacted to `~`)

```jsonl
{"requirement_id":"PKG-09","baseline_source":"packages/opencode/src/index.ts:51 (yargs .version), packages/core/src/installation/version.ts:6","owner":"phase-09 (bins/opencode CLI)","producer_consumer":"opencode CLI -> user shell / install scripts","workflow":"cli-version (baseline)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"git-tree:212e389ea86ec333412dccb199145f7542b5c557","command_run_id":"BASELINE_CMD=bun run src/index.ts run=1791028266-27117","expected":["cli [\"--version\"] exit=0 stdout~^(local|\\d+\\.\\d+\\.\\d+\\S*)$"],"observed":["cli [\"--version\"] exit=Some(0) stdout=\"local\" 1980ms"],"reviewer":"unreviewed","gate_decision":"baseline-recorded"}
{"requirement_id":"PKG-04","baseline_source":"packages/opencode/src/cli/cmd/serve.ts, packages/opencode/src/server/routes/instance/httpapi/groups/global.ts, packages/protocol/src/groups/health.ts","owner":"phase-06 (protocol/server), phase-91 (composition)","producer_consumer":"opencode serve HTTP API -> TUI / app / SDK clients","workflow":"serve-health (baseline)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"git-tree:212e389ea86ec333412dccb199145f7542b5c557","command_run_id":"BASELINE_CMD=bun run src/index.ts run=1791028266-27117","expected":["serve ready: GET /global/health 200","GET /global/health status=200 json={\"/healthy\":true} types={\"/version\":\"string\"} array_item={}","sse global /global/event first=server.connected"],"observed":["serve healthy in 3416ms","GET /global/health -> 200 {\"healthy\":true,\"version\":\"local\"}","sse global first event server.connected"],"reviewer":"unreviewed","gate_decision":"baseline-recorded"}
{"requirement_id":"PKG-04","baseline_source":"packages/opencode/src/server/routes/instance/httpapi/groups/session.ts, packages/opencode/src/session/index.ts","owner":"phase-04 (sessions), phase-06 (protocol/server)","producer_consumer":"opencode serve /session + /event -> TUI / app / SDK clients","workflow":"session-create-list (baseline)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"git-tree:212e389ea86ec333412dccb199145f7542b5c557","command_run_id":"BASELINE_CMD=bun run src/index.ts run=1791028266-27117","expected":["serve ready: GET /global/health 200","sse dir /event?directory=${workdir} first=server.connected","POST /session?directory=${workdir} status=200 json={\"/cost\":0,\"/directory\":\"${workdir_real}\",\"/tokens/input\":0} types={\"/id\":\"string\",\"/projectID\":\"string\",\"/slug\":\"string\",\"/time/created\":\"number\",\"/time/updated\":\"number\",\"/title\":\"string\"} array_item={}","sse dir event session.created json={\"/properties/info/id\":\"${session_id}\",\"/properties/sessionID\":\"${session_id}\"}","GET /session?directory=${workdir} status=200 json={} types={} array_item={\"/directory\":\"${workdir_real}\",\"/id\":\"${session_id}\"}","GET /session/${session_id}?directory=${workdir} status=200 json={\"/id\":\"${session_id}\"} types={} array_item={}"],"observed":["serve healthy in 9328ms","sse dir first event server.connected","POST /session?directory=~/.jcode/scratch/opencode-p15-work-TAKWBm -> 200 {\"id\":\"ses_efe6537f3ffeB5Oy9PycPOIo2f\",\"slug\":\"swift-knight\",\"projectID\":\"global\",\"directory\":\"~/.jcode/scratch/opencode-p15-work-TAKWBm\",\"path\":\"\",\"cost\":0,\"tokens\":{\"input\":0,\"output\":0,\"reasoning\":0,\"cache\":{\"read\":0,\"write\"...","sse dir saw session.created","GET /session?directory=~/.jcode/scratch/opencode-p15-work-TAKWBm -> 200 [{\"id\":\"ses_efe6537f3ffeB5Oy9PycPOIo2f\",\"slug\":\"swift-knight\",\"projectID\":\"global\",\"directory\":\"~/.jcode/scratch/opencode-p15-work-TAKWBm\",\"path\":\"\",\"cost\":0,\"tokens\":{\"input\":0,\"output\":0,\"reasoning\":0,\"cache\":{\"read\":0,\"write...","GET /session/ses_efe6537f3ffeB5Oy9PycPOIo2f?directory=~/.jcode/scratch/opencode-p15-work-TAKWBm -> 200 {\"id\":\"ses_efe6537f3ffeB5Oy9PycPOIo2f\",\"slug\":\"swift-knight\",\"projectID\":\"global\",\"directory\":\"~/.jcode/scratch/opencode-p15-work-TAKWBm\",\"path\":\"\",\"cost\":0,\"tokens\":{\"input\":0,\"output\":0,\"reasoning\":0,\"cache\":{\"read\":0,\"write\"..."],"reviewer":"unreviewed","gate_decision":"baseline-recorded"}
{"requirement_id":"PKG-09","baseline_source":"packages/opencode/src/index.ts:51 (yargs .version), packages/core/src/installation/version.ts:6","owner":"phase-09 (bins/opencode CLI)","producer_consumer":"opencode CLI -> user shell / install scripts","workflow":"cli-version (candidate)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"none","command_run_id":"CANDIDATE_CMD=<unset> run=1791028266-27117","expected":["cli [\"--version\"] exit=0 stdout~^(local|\\d+\\.\\d+\\.\\d+\\S*)$"],"observed":["not run"],"reviewer":"unreviewed","gate_decision":"blocked: CANDIDATE_CMD binary not built or command unset"}
{"requirement_id":"PKG-04","baseline_source":"packages/opencode/src/cli/cmd/serve.ts, packages/opencode/src/server/routes/instance/httpapi/groups/global.ts, packages/protocol/src/groups/health.ts","owner":"phase-06 (protocol/server), phase-91 (composition)","producer_consumer":"opencode serve HTTP API -> TUI / app / SDK clients","workflow":"serve-health (candidate)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"none","command_run_id":"CANDIDATE_CMD=<unset> run=1791028266-27117","expected":["serve ready: GET /global/health 200","GET /global/health status=200 json={\"/healthy\":true} types={\"/version\":\"string\"} array_item={}","sse global /global/event first=server.connected"],"observed":["not run"],"reviewer":"unreviewed","gate_decision":"blocked: CANDIDATE_CMD binary not built or command unset"}
{"requirement_id":"PKG-04","baseline_source":"packages/opencode/src/server/routes/instance/httpapi/groups/session.ts, packages/opencode/src/session/index.ts","owner":"phase-04 (sessions), phase-06 (protocol/server)","producer_consumer":"opencode serve /session + /event -> TUI / app / SDK clients","workflow":"session-create-list (candidate)","fixture_live":"live","target":"aarch64-macos","candidate_hash":"none","command_run_id":"CANDIDATE_CMD=<unset> run=1791028266-27117","expected":["serve ready: GET /global/health 200","sse dir /event?directory=${workdir} first=server.connected","POST /session?directory=${workdir} status=200 json={\"/cost\":0,\"/directory\":\"${workdir_real}\",\"/tokens/input\":0} types={\"/id\":\"string\",\"/projectID\":\"string\",\"/slug\":\"string\",\"/time/created\":\"number\",\"/time/updated\":\"number\",\"/title\":\"string\"} array_item={}","sse dir event session.created json={\"/properties/info/id\":\"${session_id}\",\"/properties/sessionID\":\"${session_id}\"}","GET /session?directory=${workdir} status=200 json={} types={} array_item={\"/directory\":\"${workdir_real}\",\"/id\":\"${session_id}\"}","GET /session/${session_id}?directory=${workdir} status=200 json={\"/id\":\"${session_id}\"} types={} array_item={}"],"observed":["not run"],"reviewer":"unreviewed","gate_decision":"blocked: CANDIDATE_CMD binary not built or command unset"}
```

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Candidate rows | all 3 workflows against Rust | `bins/opencode` lacks `serve`, HTTP API and SSE | phase-09 / phase-06 / phase-91 | `CANDIDATE_CMD=<rust binary>` run with 3/3 `pass`, then human review |
| Reviewer field | independent review | harness never self-reviews | phase-92 | reviewer name + gate decision per row |
| Broader workflows | prompt/provider, tools, permissions, PTY, MCP, auth, error paths | need provider fixtures and per-phase expectations | owning phases + phase-15 | add workflow TOMLs per coverage ID |
| Linux arm64 rows | same workflows in Docker `linux/arm64` | needs a Linux release artifact (see `rust/tests/platform`) | phase-15 / phase-91 | platform runner row `pass` plus acceptance in container |
