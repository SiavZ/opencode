//! Purpose: criterion process-level benchmarks of user-visible workflows (CLI `--version`/`--help`
//! warm startup, `serve` spawn-to-healthy, session create to SSE `session.created`) against the
//! binary named by OPENCODE_BENCH_BIN (TypeScript dev command or Rust build). Skips when unset.
//! Scope: Phase 15 performance evidence harness, compares baseline and candidate by re-running
//! with a different OPENCODE_BENCH_BIN and criterion `--save-baseline`/`--baseline`.
//! Uses:
//! rust/benchmarks/
//! |-- src/lib.rs
//! `-- src/sse.rs
//! External:
//! |-- criterion::{Criterion, criterion_group, criterion_main}
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: every process runs with an isolated temp HOME/XDG; no provider credentials; the
//! server for the SSE bench is started once and reused; skipping is printed, never silent.
//! Contract tests: rust/contracts/phase-15/baselines.md (criterion run record)

use criterion::{Criterion, criterion_group, criterion_main};
use opencode_benchmarks::{Home, Target, agent, sse};
use serde_json::Value;
use std::time::{Duration, Instant};

const VAR: &str = "OPENCODE_BENCH_BIN";

fn target() -> Option<Target> {
    let t = Target::from_env(VAR);
    if t.is_none() {
        eprintln!(
            "SKIP opencode process benches: {VAR} is unset. Example: \
             {VAR}='bun run src/index.ts' {VAR}_CWD=$PWD/packages/opencode cargo bench"
        );
    }
    t
}

fn cli(c: &mut Criterion) {
    let Some(t) = target() else { return };
    let home = Home::new().expect("home");
    let mut g = c.benchmark_group("cli_warm");
    g.sample_size(10).measurement_time(Duration::from_secs(20));
    for args in [["--version"], ["--help"]] {
        // Prime caches so these are warm samples, cold samples live in baseline.sh.
        let (out, _) = t.run(&home, &args).expect("prime");
        assert!(out.status.success(), "{args:?} failed: {out:?}");
        g.bench_function(args[0].trim_start_matches('-'), |b| {
            b.iter(|| {
                let (out, _) = t.run(&home, &args).expect("run");
                assert!(out.status.success());
            })
        });
    }
    g.finish();
}

fn serve_ready(c: &mut Criterion) {
    let Some(t) = target() else { return };
    let home = Home::new().expect("home");
    drop(
        t.serve(&home, Duration::from_secs(60))
            .expect("prime serve"),
    );
    let mut g = c.benchmark_group("serve");
    g.sample_size(10).measurement_time(Duration::from_secs(40));
    g.bench_function("spawn_to_healthy_warm", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    t.serve(&home, Duration::from_secs(60))
                        .expect("serve")
                        .ready
                })
                .sum()
        })
    });
    g.finish();
}

fn sse_latency(c: &mut Criterion) {
    let Some(t) = target() else { return };
    let home = Home::new().expect("home");
    let work = tempfile::tempdir().expect("workdir");
    let server = t.serve(&home, Duration::from_secs(60)).expect("serve");
    let q = format!("?directory={}", work.path().display());
    let stream = sse::SseStream::connect(server.port, &format!("/event{q}")).expect("sse");
    stream
        .wait_for(0, Duration::from_secs(10), |v| {
            sse::event_type(v) == Some("server.connected")
        })
        .expect("server.connected");
    let http = agent(Duration::from_secs(10));
    let url = server.url(&format!("/session{q}"));
    let mut g = c.benchmark_group("sse");
    g.sample_size(30);
    g.bench_function("session_create_to_event", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let from = stream.len();
                    let start = Instant::now();
                    let body: Value = http
                        .post(&url)
                        .send_json(serde_json::json!({}))
                        .expect("post")
                        .body_mut()
                        .read_json()
                        .expect("json");
                    let id = body["id"].as_str().expect("id").to_owned();
                    let (at, _) = stream
                        .wait_for(from, Duration::from_secs(10), |v| {
                            sse::event_type(v) == Some("session.created")
                                && v["properties"]["sessionID"] == id.as_str()
                        })
                        .expect("session.created");
                    at - start
                })
                .sum()
        })
    });
    g.finish();
}

criterion_group!(benches, cli, serve_ready, sse_latency);
criterion_main!(benches);
