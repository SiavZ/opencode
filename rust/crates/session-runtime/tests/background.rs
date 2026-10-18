//! Purpose: verify BackgroundJobs start/extend/wait/list and wire shape against
//! packages/core/src/background-job.ts and packages/core/test/background-job.test.ts.
//! Scope: live tokio runtimes (current-thread, multi-thread, paused virtual time). No fixtures or mocks.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! `-- session-runtime/tests/background/helpers.rs
//! External:
//! |-- futures::future::FutureExt
//! |-- serde_json::json
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests observe behavior only through the public API and side effects of the submitted
//! runs and hooks. Expected values come from the baseline source and a recorded baseline probe
//! (interrupt-only error text, shutdown leaving status `running`).
//! Contract tests: session-runtime/tests/background.rs, session-runtime/tests/background_control.rs

#[path = "background/helpers.rs"]
mod helpers;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::FutureExt;
use helpers::{gated, input, metadata, never_with_flag, ok};
use opencode_session_runtime::background::{
    BackgroundJobs, JobInfo, JobStatus, StartInput, WaitResult,
};
use serde_json::json;
use tokio::sync::oneshot;

#[tokio::test]
async fn tracks_work_through_explicit_observation() {
    let jobs = BackgroundJobs::new();
    let (gate, run) = gated();
    let job = jobs.start(StartInput {
        metadata: metadata(json!({ "durable": false })),
        ..input(None, run)
    });
    assert!(job.id.starts_with("job_"), "{}", job.id);
    assert_eq!(job.id.len(), "job_".len() + 26);
    assert_eq!(job.status, JobStatus::Running);
    assert_eq!(job.job_type, "test");
    assert_eq!(job.metadata, metadata(json!({ "durable": false })));

    let immediate = jobs.wait(&job.id, Some(Duration::ZERO)).await;
    assert!(immediate.timed_out);
    assert_eq!(immediate.info.unwrap().status, JobStatus::Running);

    gate.send(Ok("done".into())).unwrap();
    let done = jobs.wait(&job.id, None).await;
    assert!(!done.timed_out);
    let info = done.info.unwrap();
    assert_eq!(info.status, JobStatus::Completed);
    assert_eq!(info.output.as_deref(), Some("done"));
    assert!(info.completed_at.unwrap() >= info.started_at);
    assert_eq!(info.error, None);
    assert_eq!(jobs.get(&job.id), Some(info));
}

#[tokio::test]
async fn publishes_jobs_before_starting_immediately_settling_work() {
    let jobs = BackgroundJobs::new();
    for index in 0..100 {
        let id = format!("job_immediate_start_{index}");
        let (probe, probe_id) = (jobs.clone(), id.clone());
        let run = async move {
            match probe.get(&probe_id) {
                Some(info) if info.status == JobStatus::Running => Ok(format!("done-{index}")),
                _ => Err("job started before publish".into()),
            }
        }
        .boxed();
        jobs.start(input(Some(&id), run));
        let info = jobs.wait(&id, None).await.info.unwrap();
        assert_eq!(info.status, JobStatus::Completed, "{info:?}");
        assert_eq!(info.output, Some(format!("done-{index}")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishes_jobs_before_work_on_a_multi_thread_runtime() {
    let jobs = BackgroundJobs::new();
    for index in 0..100 {
        let id = format!("job_mt_{index}");
        let (probe, probe_id) = (jobs.clone(), id.clone());
        let run = async move {
            probe
                .get(&probe_id)
                .filter(|info| info.status == JobStatus::Running)
                .map(|_| "ok".to_owned())
                .ok_or_else(|| "job started before publish".to_owned())
        }
        .boxed();
        jobs.start(input(Some(&id), run));
        let extended = jobs.extend(&id, ok("second"));
        let info = jobs.wait(&id, None).await.info.unwrap();
        if extended {
            assert_eq!(info.output.as_deref(), Some("second"), "{info:?}");
        }
        assert_eq!(info.status, JobStatus::Completed, "{info:?}");
    }
}

#[tokio::test]
async fn increments_pending_work_before_immediately_settling_extensions() {
    let jobs = BackgroundJobs::new();
    for index in 0..100 {
        let (gate, run) = gated();
        let job = jobs.start(input(None, run));
        assert!(jobs.extend(&job.id, ok(&format!("second-{index}"))));
        tokio::task::yield_now().await;
        assert_eq!(jobs.get(&job.id).unwrap().status, JobStatus::Running);
        gate.send(Ok(format!("first-{index}"))).unwrap();
        let info = jobs.wait(&job.id, None).await.info.unwrap();
        assert_eq!(info.status, JobStatus::Completed);
        assert_eq!(info.output, Some(format!("second-{index}")));
    }
}

#[tokio::test]
async fn running_id_is_not_started_twice() {
    let jobs = BackgroundJobs::new();
    let (_gate, run) = gated();
    let first = jobs.start(StartInput {
        title: Some("first".into()),
        ..input(Some("job_dup"), run)
    });
    let second_ran = Arc::new(AtomicBool::new(false));
    let flag = second_ran.clone();
    let second = jobs.start(StartInput {
        title: Some("second".into()),
        ..input(
            Some("job_dup"),
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok("x".to_owned())
            }
            .boxed(),
        )
    });
    tokio::task::yield_now().await;
    assert_eq!(second, first);
    assert_eq!(second.title.as_deref(), Some("first"));
    assert!(!second_ran.load(Ordering::SeqCst));
    assert_eq!(jobs.list().len(), 1);
}

#[tokio::test]
async fn extensions_run_sequentially_after_the_previous_tail() {
    let jobs = BackgroundJobs::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let step = |name: &'static str, gate: oneshot::Receiver<()>| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(format!("{name}:start"));
            let _ = gate.await;
            log.lock().unwrap().push(format!("{name}:end"));
            Ok(name.to_owned())
        }
        .boxed()
    };
    let (g1, r1) = oneshot::channel();
    let (g2, r2) = oneshot::channel();
    let (g3, r3) = oneshot::channel();
    let job = jobs.start(input(None, step("one", r1)));
    assert!(jobs.extend(&job.id, step("two", r2)));
    assert!(jobs.extend(&job.id, step("three", r3)));
    // Release later gates first: order must still follow the tails.
    g3.send(()).unwrap();
    g2.send(()).unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(*log.lock().unwrap(), vec!["one:start"]);
    g1.send(()).unwrap();
    let info = jobs.wait(&job.id, None).await.info.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "one:start",
            "one:end",
            "two:start",
            "two:end",
            "three:start",
            "three:end"
        ]
    );
    assert_eq!(info.status, JobStatus::Completed);
    assert_eq!(info.output.as_deref(), Some("three"));
}

#[tokio::test]
async fn failure_keeps_newest_successful_output_and_interrupts_queued_runs() {
    let jobs = BackgroundJobs::new();
    let (g1, r1) = gated();
    let (g2, r2) = gated();
    let job = jobs.start(input(None, r1));
    assert!(jobs.extend(&job.id, r2));
    let third_ran = Arc::new(AtomicBool::new(false));
    let (third_dropped, third) = {
        let ran = third_ran.clone();
        let (dropped, inner) = never_with_flag();
        (
            dropped,
            async move {
                ran.store(true, Ordering::SeqCst);
                inner.await
            }
            .boxed(),
        )
    };
    assert!(jobs.extend(&job.id, third));
    g1.send(Ok("first".into())).unwrap();
    g2.send(Err("second failed".into())).unwrap();
    let info = jobs.wait(&job.id, None).await.info.unwrap();
    assert_eq!(info.status, JobStatus::Error);
    assert_eq!(info.error.as_deref(), Some("second failed"));
    assert_eq!(info.output.as_deref(), Some("first"));
    // A queued run of a finished job never starts, and further extensions are refused.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(!third_ran.load(Ordering::SeqCst));
    assert!(third_dropped.load(Ordering::SeqCst));
    assert!(!jobs.extend(&job.id, ok("late")));
    assert_eq!(jobs.get(&job.id), Some(info));
}

#[tokio::test]
async fn first_run_failure_has_no_output() {
    let jobs = BackgroundJobs::new();
    let job = jobs.start(input(None, async { Err("bad".to_owned()) }.boxed()));
    let info = jobs.wait(&job.id, None).await.info.unwrap();
    assert_eq!(info.status, JobStatus::Error);
    assert_eq!(info.error.as_deref(), Some("bad"));
    assert_eq!(info.output, None);
    assert_eq!(
        serde_json::to_value(&info)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec![
            "id",
            "type",
            "status",
            "started_at",
            "completed_at",
            "error"
        ]
    );
}

#[tokio::test]
async fn panicking_run_settles_as_error_like_a_defect() {
    let jobs = BackgroundJobs::new();
    let job = jobs.start(input(
        None,
        async {
            if true {
                panic!("defect");
            }
            Ok(String::new())
        }
        .boxed(),
    ));
    let info = jobs.wait(&job.id, None).await.info.unwrap();
    assert_eq!(info.status, JobStatus::Error);
    assert_eq!(info.error.as_deref(), Some("defect"));
}

#[tokio::test(start_paused = true)]
async fn wait_timeout_returns_the_snapshot_taken_when_waiting_began() {
    let jobs = BackgroundJobs::new();
    let (gate, run) = gated();
    let job = jobs.start(input(Some("job_wait"), run));
    let waiter = {
        let jobs = jobs.clone();
        tokio::spawn(async move { jobs.wait("job_wait", Some(Duration::from_millis(20))).await })
    };
    tokio::task::yield_now().await;
    jobs.promote(&job.id).await.unwrap();
    let waited = waiter.await.unwrap();
    assert!(waited.timed_out);
    let info = waited.info.unwrap();
    assert_eq!(info.status, JobStatus::Running);
    assert_eq!(
        info.metadata, None,
        "baseline returns the pre-wait snapshot"
    );

    let within = {
        let jobs = jobs.clone();
        tokio::spawn(async move { jobs.wait("job_wait", Some(Duration::from_secs(5))).await })
    };
    tokio::task::yield_now().await;
    gate.send(Ok("late".into())).unwrap();
    let waited = within.await.unwrap();
    assert!(!waited.timed_out);
    assert_eq!(waited.info.unwrap().output.as_deref(), Some("late"));
}

#[tokio::test]
async fn wait_on_unknown_and_finished_jobs_returns_immediately() {
    let jobs = BackgroundJobs::new();
    assert_eq!(
        jobs.wait("job_missing", None).await,
        WaitResult {
            info: None,
            timed_out: false
        }
    );
    let job = jobs.start(input(None, ok("x")));
    let first = jobs.wait(&job.id, None).await;
    let again = jobs.wait(&job.id, Some(Duration::ZERO)).await;
    assert!(!again.timed_out, "finished jobs never time out");
    assert_eq!(again, first);
}

#[tokio::test]
async fn list_sorts_by_start_time_and_snapshots_are_detached() {
    let jobs = BackgroundJobs::new();
    let (_g1, r1) = gated();
    jobs.start(StartInput {
        metadata: metadata(json!({ "k": 1 })),
        ..input(Some("job_b"), r1)
    });
    tokio::time::sleep(Duration::from_millis(3)).await;
    let (_g2, r2) = gated();
    jobs.start(input(Some("job_a"), r2));
    let ids: Vec<_> = jobs.list().into_iter().map(|job| job.id).collect();
    assert_eq!(ids, vec!["job_b", "job_a"]);

    let mut snapshot = jobs.get("job_b").unwrap();
    snapshot
        .metadata
        .as_mut()
        .unwrap()
        .insert("k".into(), json!(2));
    assert_eq!(
        jobs.get("job_b").unwrap().metadata,
        metadata(json!({ "k": 1 }))
    );
}

#[test]
fn info_wire_shape_matches_baseline_key_order_and_casing() {
    let info: JobInfo = serde_json::from_value(json!({
        "id": "job_1",
        "type": "task",
        "title": "t",
        "status": "cancelled",
        "started_at": 1,
        "metadata": { "background": true },
        "completed_at": 2,
        "output": "o",
        "error": "All fibers interrupted without error"
    }))
    .unwrap();
    assert_eq!(info.status, JobStatus::Cancelled);
    assert!(info.is_background());
    let wire = serde_json::to_string(&info).unwrap();
    assert_eq!(
        wire,
        r#"{"id":"job_1","type":"task","title":"t","status":"cancelled","started_at":1,"metadata":{"background":true},"completed_at":2,"output":"o","error":"All fibers interrupted without error"}"#
    );
    let minimal: JobInfo = serde_json::from_value(
        json!({ "id": "j", "type": "x", "status": "running", "started_at": 5 }),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&minimal).unwrap(),
        r#"{"id":"j","type":"x","status":"running","started_at":5}"#
    );
}
