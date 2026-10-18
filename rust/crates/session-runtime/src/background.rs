//! Purpose: process-local background job registry (`BackgroundJob`), porting
//! packages/core/src/background-job.ts: start/extend/wait/waitForPromotion/promote/cancel/list/get.
//! Scope: one registry per owner (process-global in the baseline). Jobs are intentionally not durable:
//! restart or `shutdown` loses status and interrupts live work, like the baseline.
//! Uses:
//! rust/crates/
//! |-- schema/src/identifier.rs
//! `-- session-runtime/src/todo.rs
//! External:
//! |-- futures::future::{BoxFuture, FutureExt}
//! |-- indexmap::IndexMap
//! |-- serde::{Deserialize, Serialize}
//! |-- tokio::sync::watch
//! `-- tokio_util::{sync::CancellationToken, task::TaskTracker}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/tests/background.rs
//! |-- session-runtime/tests/background/helpers.rs
//! `-- session-runtime/tests/background_control.rs
//! Invariants: a running ID is never started twice (start returns the running snapshot). A job is
//! published before its first run is polled. Extended runs execute sequentially after the previous
//! tail; the newest successful output (by sequence) wins; the job completes when the last pending run
//! succeeds, or at the first failure (queued runs are interrupted). Settlement from a superseded
//! generation is ignored. Every task is tracked by the job's TaskTracker (joined by `cancel`) and the
//! registry TaskTracker (joined by `shutdown`). `shutdown` interrupts work without settling it: status
//! stays `running` and pending waiters observe `info: None`. `start`/`extend` must run inside a tokio
//! runtime, and `cancel` must not be awaited from inside the job's own run or hooks (it joins them).
//! Contract tests: session-runtime/tests/background.rs, session-runtime/tests/background_control.rs

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt};
use indexmap::IndexMap;
use opencode_schema::identifier::{self, Direction};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::todo::now_millis;

/// `Cause.squash` text of an interrupt-only cause in the baseline Effect runtime.
const INTERRUPTED: &str = "All fibers interrupted without error";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Error,
    Cancelled,
}

/// `BackgroundJob.Info` (snake_case keys, like the baseline). Field order follows the key order the
/// baseline produces: `id, type, title, status, started_at, metadata, completed_at, output, error`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub job_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: JobStatus,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl JobInfo {
    /// `metadata.background === true`.
    pub fn is_background(&self) -> bool {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.get("background"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    pub fn metadata_str(&self, key: &str) -> Option<&str> {
        self.metadata.as_ref()?.get(key)?.as_str()
    }
}

/// Work of one run. `Err` carries the failure text (`errorText(Cause.squash(..))`). A panic is
/// treated like an Effect defect: the job settles as `error` with the panic message.
pub type JobRun = BoxFuture<'static, Result<String, String>>;

pub struct StartInput {
    pub id: Option<String>,
    pub job_type: String,
    pub title: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    /// Runs once when the job is promoted to background.
    pub on_promote: Option<BoxFuture<'static, ()>>,
    /// Runs when the first run is interrupted (`Effect.onInterrupt`), before the run settles.
    pub on_interrupt: Option<BoxFuture<'static, ()>>,
    pub run: JobRun,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaitResult {
    /// `None` for an unknown job, or when the registry was shut down while waiting.
    pub info: Option<JobInfo>,
    pub timed_out: bool,
}

#[derive(Clone)]
enum Done {
    Pending,
    Settled(Box<JobInfo>),
    /// Owner shutdown interrupted the job without settling it.
    Abandoned,
}

struct Active {
    info: JobInfo,
    done: watch::Sender<Done>,
    promoted: watch::Sender<Option<JobInfo>>,
    token: u64,
    cancel: CancellationToken,
    tracker: TaskTracker,
    abandoned: bool,
    pending: u32,
    next: u64,
    output: Option<(u64, String)>,
    tail: watch::Receiver<bool>,
    on_promote: Option<BoxFuture<'static, ()>>,
}

impl Active {
    fn snapshot(&self) -> JobInfo {
        self.info.clone()
    }

    fn running(&self) -> bool {
        self.info.status == JobStatus::Running
    }

    /// Terminal transition shared by settle and cancel. Queued runs stop being joinable work.
    fn finish(&mut self, status: JobStatus, error: Option<String>) -> JobInfo {
        self.on_promote = None;
        self.pending = 0;
        self.info.status = status;
        self.info.completed_at = Some(now_millis());
        if let Some((_, text)) = &self.output {
            self.info.output = Some(text.clone());
        }
        if error.is_some() {
            self.info.error = error;
        }
        let info = self.snapshot();
        self.done
            .send_replace(Done::Settled(Box::new(info.clone())));
        self.tracker.close();
        info
    }
}

#[derive(Default)]
struct State {
    jobs: IndexMap<String, Active>,
    generation: u64,
}

/// `BackgroundJob.Service`.
#[derive(Clone, Default)]
pub struct BackgroundJobs {
    state: Arc<Mutex<State>>,
    tasks: TaskTracker,
}

enum Exit {
    Success(String),
    Failure(String),
    Interrupted,
}

/// One spawned run: the gate it waits for and the hooks around it.
struct Run {
    id: String,
    token: u64,
    sequence: u64,
    previous: Option<watch::Receiver<bool>>,
    tail: watch::Sender<bool>,
    on_interrupt: Option<BoxFuture<'static, ()>>,
    work: JobRun,
}

impl BackgroundJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshots sorted by `started_at` ascending. Ties keep first-insertion order of the ID, like
    /// a stable sort over the baseline `Map`.
    pub fn list(&self) -> Vec<JobInfo> {
        let mut jobs: Vec<_> = self.lock().jobs.values().map(Active::snapshot).collect();
        jobs.sort_by_key(|job| job.started_at);
        jobs
    }

    pub fn get(&self, id: &str) -> Option<JobInfo> {
        self.lock().jobs.get(id).map(Active::snapshot)
    }

    /// Start a job, or return the running snapshot when the ID is already running. A finished job
    /// with the same ID is replaced and its late settlements are ignored.
    pub fn start(&self, input: StartInput) -> JobInfo {
        let id = input
            .id
            .unwrap_or_else(|| identifier::create("job", Direction::Ascending, None));
        let mut state = self.lock();
        if let Some(existing) = state.jobs.get(&id)
            && existing.running()
        {
            return existing.snapshot();
        }
        state.generation += 1;
        let token = state.generation;
        let (tail, tail_rx) = watch::channel(false);
        let job = Active {
            info: JobInfo {
                id: id.clone(),
                job_type: input.job_type,
                title: input.title,
                status: JobStatus::Running,
                started_at: now_millis(),
                metadata: input.metadata,
                completed_at: None,
                output: None,
                error: None,
            },
            done: watch::channel(Done::Pending).0,
            promoted: watch::channel(None).0,
            token,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
            abandoned: false,
            pending: 1,
            next: 1,
            output: None,
            tail: tail_rx,
            on_promote: input.on_promote,
        };
        let info = job.snapshot();
        // Spawned under the lock after insertion, so the run can only observe the published job.
        self.spawn(
            &job,
            Run {
                id: id.clone(),
                token,
                sequence: 0,
                previous: None,
                tail,
                on_interrupt: input.on_interrupt,
                work: input.run,
            },
        );
        state.jobs.insert(id, job);
        info
    }

    /// Queue another run after the current tail. Returns false when the job is not running.
    pub fn extend(&self, id: &str, run: JobRun) -> bool {
        let mut state = self.lock();
        let Some(job) = state.jobs.get_mut(id) else {
            return false;
        };
        if !job.running() {
            return false;
        }
        let (tail, tail_rx) = watch::channel(false);
        let previous = std::mem::replace(&mut job.tail, tail_rx);
        let sequence = job.next;
        job.pending += 1;
        job.next += 1;
        self.spawn(
            job,
            Run {
                id: id.to_owned(),
                token: job.token,
                sequence,
                previous: Some(previous),
                tail,
                on_interrupt: None,
                work: run,
            },
        );
        true
    }

    /// Wait for completion. `None` waits forever. A zero timeout returns immediately with
    /// `timed_out`. On timeout the snapshot taken when the wait began is returned, like the baseline.
    pub async fn wait(&self, id: &str, timeout: Option<Duration>) -> WaitResult {
        let result = |info, timed_out| WaitResult { info, timed_out };
        let (snapshot, receiver) = {
            let state = self.lock();
            let Some(job) = state.jobs.get(id) else {
                return result(None, false);
            };
            if !job.running() {
                return result(Some(job.snapshot()), false);
            }
            if timeout.is_some_and(|timeout| timeout.is_zero()) {
                return result(Some(job.snapshot()), true);
            }
            (job.snapshot(), job.done.subscribe())
        };
        let Some(timeout) = timeout else {
            return result(settled(receiver).await, false);
        };
        match tokio::time::timeout(timeout, settled(receiver)).await {
            Ok(info) => result(info, false),
            Err(_) => result(Some(snapshot), true),
        }
    }

    /// Resolves when the job is promoted to background. Never resolves for unknown or finished jobs.
    pub async fn wait_for_promotion(&self, id: &str) -> JobInfo {
        let receiver = {
            let state = self.lock();
            match state.jobs.get(id) {
                Some(job) if job.running() => {
                    if job.info.is_background() {
                        return job.snapshot();
                    }
                    Some(job.promoted.subscribe())
                }
                _ => None,
            }
        };
        if let Some(mut receiver) = receiver
            && let Ok(info) = receiver.wait_for(Option::is_some).await
            && let Some(info) = info.clone()
        {
            return info;
        }
        std::future::pending().await
    }

    /// Mark a running job as background (`metadata.background = true`) and run its `on_promote` once.
    pub async fn promote(&self, id: &str) -> Option<JobInfo> {
        let (info, hook) = {
            let mut state = self.lock();
            let job = state.jobs.get_mut(id)?;
            if !job.running() {
                return None;
            }
            if job.info.is_background() {
                return Some(job.snapshot());
            }
            job.info
                .metadata
                .get_or_insert_with(Map::new)
                .insert("background".into(), Value::Bool(true));
            let info = job.snapshot();
            job.promoted.send_replace(Some(info.clone()));
            (info, job.on_promote.take())
        };
        if let Some(hook) = hook {
            hook.await;
        }
        Some(info)
    }

    /// Cancel a running job, interrupt its runs and join them. Finished jobs return their snapshot.
    pub async fn cancel(&self, id: &str) -> Option<JobInfo> {
        let (info, cancel, join) = {
            let mut state = self.lock();
            let job = state.jobs.get_mut(id)?;
            if !job.running() {
                return Some(job.snapshot());
            }
            let info = job.finish(JobStatus::Cancelled, None);
            // An abandoned job's tasks belong to the shutdown join, which may be our caller.
            let join = (!job.abandoned).then(|| job.tracker.clone());
            (info, job.cancel.clone(), join)
        };
        cancel.cancel();
        if let Some(tracker) = join {
            tracker.wait().await;
        }
        Some(info)
    }

    /// Owner-scope closure: interrupt every job and join every task. Running jobs are not settled
    /// (status stays `running`), and their pending waiters resolve with `info: None`.
    pub async fn shutdown(&self) {
        for job in self.lock().jobs.values_mut() {
            if job.running() {
                job.abandoned = true;
                job.done.send_replace(Done::Abandoned);
            }
            job.cancel.cancel();
        }
        self.tasks.close();
        self.tasks.wait().await;
        self.tasks.reopen();
    }

    /// Spawn one run on both trackers. Called with the state lock held so publication and pending
    /// counts are visible before the run is polled.
    fn spawn(&self, job: &Active, run: Run) {
        let registry = self.clone();
        let cancel = job.cancel.clone();
        let task = async move {
            let previous = run.previous;
            let work = run.work;
            let exit = tokio::select! {
                biased;
                _ = cancel.cancelled() => Exit::Interrupted,
                result = async move {
                    if let Some(mut previous) = previous {
                        // A dropped sender also releases the gate, like `ensuring` on the tail.
                        let _ = previous.wait_for(|finished| *finished).await;
                    }
                    AssertUnwindSafe(work).catch_unwind().await
                } => match result {
                    Ok(Ok(output)) => Exit::Success(output),
                    Ok(Err(error)) => Exit::Failure(error),
                    Err(panic) => Exit::Failure(panic_text(panic)),
                },
            };
            if let (Exit::Interrupted, Some(hook)) = (&exit, run.on_interrupt) {
                hook.await;
            }
            // Settle before releasing the tail, so a failure interrupts the next queued run before it
            // can start (the baseline only closes the scope after a scheduler hop).
            registry.settle(&run.id, run.token, run.sequence, exit);
            let _ = run.tail.send(true);
        };
        self.tasks.spawn(job.tracker.track_future(task));
    }

    fn settle(&self, id: &str, token: u64, sequence: u64, exit: Exit) {
        let mut state = self.lock();
        let Some(job) = state.jobs.get_mut(id) else {
            return;
        };
        if job.token != token || !job.running() || job.abandoned {
            return;
        }
        if let Exit::Success(text) = &exit
            && job
                .output
                .as_ref()
                .is_none_or(|(previous, _)| sequence > *previous)
        {
            job.output = Some((sequence, text.clone()));
        }
        let pending = job.pending.saturating_sub(1);
        if matches!(exit, Exit::Success(_)) && pending > 0 {
            job.pending = pending;
            return;
        }
        match exit {
            Exit::Success(_) => job.finish(JobStatus::Completed, None),
            Exit::Failure(error) => job.finish(JobStatus::Error, Some(error)),
            Exit::Interrupted => job.finish(JobStatus::Cancelled, Some(INTERRUPTED.into())),
        };
        // Interrupt queued runs of a finished job (baseline closes the job scope).
        job.cancel.cancel();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

async fn settled(mut receiver: watch::Receiver<Done>) -> Option<JobInfo> {
    let done = receiver.wait_for(|done| !matches!(done, Done::Pending));
    match &*done.await.ok()? {
        Done::Settled(info) => Some((**info).clone()),
        Done::Pending | Done::Abandoned => None,
    }
}

/// `errorText(defect)`: string payloads keep their text.
fn panic_text(panic: Box<dyn Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Unknown panic".into())
}
