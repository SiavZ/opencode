# Runbook: telemetry exporter failure

Baseline: OTLP export is opt-in through `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS` (comma-separated `k=v`) and `OTEL_RESOURCE_ATTRIBUTES` (`packages/core/src/observability/otlp.ts`, `packages/core/src/flag/flag.ts:16-17`). Resource: `service.name=opencode`, `deployment.environment.name=<channel>`, `service.instance.id=<runID>`. In the Rust port this is owned by `rust/crates/observability`.

## Required property

Exporter failure must never block or fail user work. Spans/logs are dropped through bounded queues with an overflow counter. Export errors are logged locally at most once per interval. They are not retried in a hot loop.

## Symptoms

- Collector dashboards show a gap for `service.name=opencode`.
- Local log lines such as `OTLP export failed` or `timed out`.
- Users report slowness that correlates with the endpoint being unreachable. **That would be a bug**: exporter backpressure is leaking into the request path.

## Triage

1. Confirm the configuration in the affected process: `env | grep OTEL_` (redact the header values, they often carry API keys).
2. Reachability: `curl -sS -o /dev/null -w '%{http_code}\n' "$OTEL_EXPORTER_OTLP_ENDPOINT/v1/logs" -X POST -H 'content-type: application/json' -d '{}'`. A 4xx response means the endpoint is reachable but auth or the payload is wrong. A connection error means a network or collector outage.
3. Auth: an expired collector token causes 401/403. Rotate it via `credential-rotation.md` and update `OTEL_EXPORTER_OTLP_HEADERS`.
4. Malformed `OTEL_RESOURCE_ATTRIBUTES`: the baseline silently ignores the whole value (`catch {}` returns `{}`). Check that the expected attributes are missing rather than assuming the export failed.

## Mitigation

- Collector outage: no action needed on clients. Data during the outage is lost by design (no disk spooling).
- User impact from export: unset `OTEL_EXPORTER_OTLP_ENDPOINT` and restart to disable export entirely, then file a Sev 2 bug against observability.
- Wrong endpoint or credentials: fix the env and restart. Exporter config is read at startup.

## Verification

- After the fix, a fresh run shows a new `service.instance.id` in the collector within one export interval.
- With the collector unreachable, a session prompt completes with the same latency as with export disabled.

## What is untested

All of it. The Rust observability exporter is owned by another node and was not exercised here. No collector outage drill or latency comparison has been run. The overflow counter and log rate-limit are requirements of this runbook, not verified behavior.
