# Native console workers

Baseline entrypoints are `packages/console/function/src/stat.ts` default `fetch`
and `packages/console/function/src/log-processor.ts` default `tail`.
`infra/console.ts` registers the Stat HTTP worker and LogProcessor tail worker.
There is no baseline cron handler. `StatStore::scheduled` is an explicit native
capture extension, not a claim that the baseline has a scheduler.

## Process modes

- `OPENCODE_HOSTED_SERVICE=console-stat`: standalone HTTP server, configured by
  `OPENCODE_CONSOLE_DATABASE_URL` and `OPENCODE_HOSTED_ADDR`. POST any path with
  `{"ids":["provider/model/tpsGoal"]}` returns thirty UTC minute buckets, newest
  first. Missing rows are zeros. Non-POST returns 405 `Method Not Allowed`.
- `OPENCODE_HOSTED_SERVICE=console-worker`: reads one JSON value from stdin,
  capped at 8 MiB, and executes `OPENCODE_CONSOLE_WORKER_COMMAND`. `stat` accepts
  the POST object, `log-processor` accepts a Cloudflare-format trace array, and
  `inventory` lists native operators and exact unsupported reasons.

- `OPENCODE_HOSTED_SERVICE=console-log-processor`: separate HTTP tail ingress.
  POST `/tail` accepts the baseline `TraceItem[]` array. Requires nonempty
  `OPENCODE_CONSOLE_LOG_INGRESS_TOKEN` bearer and explicit capture configuration.
  Authorization precedes parsing; body limit is 8 MiB. A successful 202 response
  means capture completed, not that an external analytics provider processed it.
  This mode needs no MySQL configuration.

The ingress is an explicit bridge contract, not a Cloudflare tail-consumer binding.
`workers::producer::TailEmitter` delivers finalized native gateway metric envelopes
with a five-second deadline, no detached tasks and no redirects. Its endpoint must
be literal loopback HTTP(S), and the token must be nonempty. Gateway lifecycle
integration is owned by the Zen module, using `OPENCODE_CONSOLE_TAIL_URL` with
`OPENCODE_CONSOLE_LOG_INGRESS_URL` alias and the shared ingress token. Request
metadata stays in the trace envelope; `request_id` becomes the baseline metric
`request` after extraction. Ordinary tracing output is not treated as a tail event.
The separate gateway acceptance test covers actual request generation through this
emitter, native ingress, and configured HTTP capture. No Cloudflare tail deployment
or live analytics provider acceptance is claimed.

No auth/Stripe/provider configuration is needed for stat or stdin log workers.
No operator is exposed through the public HTTP router.

## Explicit capture

Set exactly one of `OPENCODE_CONSOLE_WORKER_CAPTURE_FILE` for fsync-backed JSONL
or `OPENCODE_CONSOLE_WORKER_CAPTURE_URL` for JSON POST. Optional
`OPENCODE_CONSOLE_WORKER_CAPTURE_TOKEN` adds bearer authorization. HTTP capture
has a thirty-second timeout, disables redirects, and propagates non-2xx errors.
No Honeycomb endpoint is inferred, and the baseline's disabled export stays
disabled. Log output contains `console` enriched records and `events` describing
the baseline's computed, but not uploaded, Honeycomb batch records.

## Disposable operator execution

Native commands are `lookup-user`, `disable-reload`, `create-coupon`,
`credit-workspace`, `freeze-workspace`, `black-onboard-waitlist`,
`black-cancel-waitlist`, `black-select-workspaces`, `create-api-key`, `reset-db`.

CLI operator invocation requires a loopback MySQL URL, a database name starting
with `p12_`, and `OPENCODE_CONSOLE_OPERATOR_CONFIRM_DATABASE` exactly equal to
that database name. Adapter mutation calls additionally require the same
`confirmDatabase` JSON field. `reset-db` also requires `stage: "frank"`.
`create-api-key` outputs its newly provisioned secret only in its explicit result.
Protect stdout and capture files accordingly. No commands were run on real data.

Additional native operators are `black-gift`, `black-transfer`, and `black-stats`.
Gift and transfer execute real existing StripeClient requests against only the
explicit `stripeCaptureUrl` literal-loopback destination, followed by baseline
MySQL transactions. Gift takes `workspaceID`, `seats`, and configured `priceID`
for plan 200. Transfer takes `fromWorkspaceID` and `toWorkspaceID`.
`black-stats` takes `plan` and returns `filename`, exact CSV content, subscriber
count and formatted average, using the baseline January 31, 2026 cutoff.

The five secret commands operate through an explicit local config-store adapter,
not the SST CLI or cloud provider. `configStore` is a directory containing marker
`.console-worker-fixture` with text `p12-console-config-v1`. `confirmConfigStore`
must match its canonical path. Stage files are string-valued JSON secret objects.
`pull-models` reads 20 parts from `stage` and writes `localStage`; `promote-models`
reads 30 parts from `frank` and writes `stage`; `update-models` takes `edited` JSON
and writes 30 parts to `frank`. `update-limits` takes `edited` JSON and writes
`ZEN_LIMITS` in `fallback`; `promote-limits` copies it to `stage`. Schema validation
precedes atomic durable replacement. Unrelated secret keys are preserved.
These config operations need their store confirmation rather than a MySQL connection
confirmation. No production cloud deployment or promotion was executed.

## Validation

`crates/hosted-adapters/tests/console_workers_fixture.json` contains outputs
captured by executing the actual baseline log handler body locally. Only unused
Resource imports and the console output destination were substituted. Fixtures
cover CF enrichment, IPv4, IPv6, partial integer parsing, invalid content length,
missing request/response metadata, route filters and sequential metric snapshots.
`console_workers_stat_fixture.json` captures actual stat handler output across
a year boundary. `console_workers_black_fixture.csv` captures actual Black CSV
report transformations with local query rows and in-memory file output. The Rust
MySQL tests compare complete captured bucket arrays and CSV content, not only
hand-selected values.

`cargo test -p opencode-hosted-adapters --test console_workers --test console_workers_config --test console_workers_operators` exercises that
fixture, real loopback HTTP and durable files, isolated local baseline-migrated
MySQL, thirty-bucket queries, minute calendar boundaries and operator transactions.
`cargo test -p opencode-hosted console_workers` exercises native command and
HTTP method/input handling. Run from `rust/`, with the shared p12 target and
sccache, only after obtaining the coordinator's build slot.