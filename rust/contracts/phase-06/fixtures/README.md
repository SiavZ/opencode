# Phase 06 baseline HTTP fixtures

Compatibility evidence for `opencode-server`: request/response pairs recorded from the real
TypeScript server, replayed against the Rust router.

- Fixtures: `rust/crates/server/tests/fixtures/baseline/<family>.json` (one file per route family).
  They live in the server crate's test tree so `cargo test -p opencode-server` finds them without
  path tricks. This directory holds the rules (this file) and the generated `coverage.md`.
- Capture script: `rust/contracts/phase-06/capture/capture.py`.
- Replay: `rust/crates/server/tests/replay_baseline.rs` (+ `tests/support/replay/`).
- Expected-failure ledger: `rust/crates/server/tests/fixtures/replay-ledger.json`.
- Coverage table: `coverage.md` (generated, route -> fixture family -> replay result).

## Capture

```sh
python3 rust/contracts/phase-06/capture/capture.py      # P06_KEEP=1 keeps temp roots and logs
```

The script starts two `bun run ./src/index.ts serve` processes from `packages/opencode`:

| server | env |
| --- | --- |
| `nopass` | no `OPENCODE_SERVER_PASSWORD` |
| `pass` | `OPENCODE_SERVER_PASSWORD=fixture-password` (username default `opencode`) |

Each gets a fresh `mkdtemp` root with `HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
`XDG_CACHE_HOME` and `TMPDIR` inside it, and a temp git project (`proj/`: `README.md` committed
then modified, `src/main.ts`). Every request carries `x-opencode-directory: <proj>`. No provider
credentials exist; `OPENCODE_CONFIG_CONTENT` declares one offline `fixture` provider
(`@ai-sdk/openai-compatible`, base URL on a dead local port, placeholder key) so `noReply` prompts
can resolve a default model. `HTTP(S)_PROXY` points at a dead local port, so UI fallback, MCP and
models fetch can never reach the network. `global.upgrade` and `session.shell` happy paths are not
executed (only their schema rejections), dispose routes run last.

Cases recorded per operation in `packages/sdk/openapi.json` (188 operations, 162 paths):

- `<op>#ok`: happy path (or the baseline's natural error for missing/unavailable resources).
- `<op>#bad-body`: malformed JSON body. Baseline answers 500 `UnknownError` (the Effect JSON
  parse defect), not 400; recorded as-is.
- `<op>#wrong-type`: JSON array body, the schema rejection (current tagged `InvalidRequestError`,
  legacy NamedError `BadRequest`).
- `<op>#missing-session`, `<op>#missing-pty`: not-found envelopes.
- `<op>#missing-query`: required query omitted.
- Extra: query rejections (`bad-limit`, `bad-order`, `bad-cursor`, `bad-id`), 415 text/plain prompt,
  503 for V2 compact/wait.
- Families `negative-auth` (401 + `WWW-Authenticate`, header vs `auth_token` query, PTY ticket
  bypass, preflight before auth), `negative-cors` (allowed and disallowed origins, preflight and
  simple), `negative-notfound`, `sse` (first frames of `/api/event`, `/event`, `/global/event`,
  `/api/session/:id/event`, plus their error envelopes), `docs` (`/doc`, `/openapi.json`).

Cases carry a global `seq`: replay runs in capture order because later cases reuse ids bound by
earlier ones (e.g. `setup:v2.session.create` binds `{ses_2}`).

## Case format

```json
{
  "seq": 12, "id": "v2.session.get#ok", "operationId": "v2.session.get", "server": "nopass",
  "request": { "method": "GET", "path": "/api/session/{ses_2}", "auth": "valid",
               "query": [["limit", "5"]], "headers": {}, "body": null, "raw_body": null },
  "response": { "status": 200, "headers": { "content-type": "application/json" },
                "kind": "json", "body": { "data": { "id": "{ses_2}" } } }
}
```

`auth` is a marker (`none`, `invalid`, `valid`, `query-valid`). Credentials are never written; the
replay rebuilds them. Response `kind`: `json`, `text`, `empty`, `sse` (`frames[]` with `shape`,
`fields`, `data`), `timeout` (baseline long-poll did not answer in 3 s), `json-digest`/`text-digest`
(bodies over 48 KiB: byte length, sha256 and, for OpenAPI documents, the sorted `paths` keys).

Recorded response headers: `content-type`, `www-authenticate`, `access-control-*`, `vary`,
`content-encoding`, `cache-control`, `location`, `content-security-policy`. Date, length,
connection and server headers are dropped.

## Normalization rules

Applied by `Normalizer` in the capture script, matched by `tests/support/replay/matcher.rs`.
Nothing else is normalized: field names, key order, optional vs null, numbers and messages are
compared exactly.

| placeholder | replaces | replay matches |
| --- | --- | --- |
| `{PROJ}` | temp project directory (also percent-encoded) | the replay's temp project |
| `{TMP}` | temp root (HOME/XDG parent) | the replay's temp root |
| `{CWD}`, `{REPO}`, `{USER_HOME}` | server cwd, repo root, real home (none currently occur) | literal |
| `{<prefix>_<n>}` | `ses|msg|per|que|prt|pty|evt|wrk|job|tool|...` + 26 char id, numbered per run | binds on first sight, must then be the same id; two placeholders never bind one id |
| `{SHA}` | 40-hex (project ids, commit hashes) | `[0-9a-f]{40}` |
| `{UUID}` | UUIDs | UUID shape |
| `{ERR_REF}` | `err_xxxxxxxx` defect references | same shape |
| `{ISO}` | ISO-8601 UTC timestamps inside strings | ISO shape |
| `{TS}` | any number in [1.5e12, 2.5e12] (epoch ms) | any integer |
| `{PID}` | `pid` field | any integer |
| `{DEAD_PORT}` | the dead local proxy/provider port | digits |
| `{SLUG}`, `{TICKET}` | values of `slug` and `ticket` keys | any non-empty string |
| `{NAME}` | `experimental.projectCopy.generateName` random name | any non-empty string |
| `{CURSOR:<json>}` | base64url JSON pagination cursors (`previous`/`next`) | decoded and matched structurally |

Literal ids that are not 26-char generated ids (e.g. `ses_missing000000000000000000`) stay
literal, so not-found messages are compared exactly.

## Replay and ledger

`replay_baseline.rs` builds `app::router(Services::default(), ServerConfig::full_host(..))` for
each server variant and drives it in-process (`tower::ServiceExt::oneshot`). Every case either
passes or appears in `replay-ledger.json` with a category:

| category | meaning |
| --- | --- |
| `not-mounted` | no Rust route matched: the full host UI fallback answered 503 `ServiceUnavailableError` with `service: "ui"` |
| `not-ported` | route mounted, but Rust answered the typed 503 `ServiceUnavailableError` where the baseline served the operation (handler not ported or service not composed) |
| `status` | other status mismatch |
| `headers` | a recorded header differs or is missing/extra |
| `body` | body differs (first JSON path in the ledger detail) |
| `stream` | SSE framing or first-frame data differs |
| `timeout` | Rust did not answer where the baseline did |

The test fails when a case fails without a ledger entry, fails with another category, or passes
while ledgered (stale). After a reviewed change: `REPLAY_BLESS=1 cargo test -p opencode-server
--test replay_baseline` rewrites the ledger and `coverage.md`; the plain run then requires both to
be current. Recapturing (TS behavior change) regenerates the fixtures; check `git diff` for
nondeterminism (two consecutive captures are byte-identical today).
