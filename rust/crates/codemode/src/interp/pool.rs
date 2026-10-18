//! Purpose: eager tool-call task pool backing first-class promise values.
//! Scope: task admission under the concurrency cap, cooperative polling, race interruption,
//! settlement observation for unhandled-rejection tracking.
//! Uses:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/tools/runtime.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- futures::future::BoxFuture
//! `-- indexmap::{IndexMap, IndexSet}
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/mod.rs
//! `-- codemode/src/interp/promise.rs
//! Invariants: at most `TOOL_CALL_CONCURRENCY` host calls run at once; dropping the pool drops
//! (interrupts) every in-flight host future; a settled task is never re-run.
//! Contract tests: codemode/tests/promise.rs

use crate::error::{Eval, Fault};
use crate::tools::runtime::ToolRuntime;
use crate::value::Value;
use futures::FutureExt;
use futures::future::BoxFuture;
use indexmap::{IndexMap, IndexSet};
use std::collections::VecDeque;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Maximum number of eagerly started tool calls that may run concurrently.
pub(crate) const TOOL_CALL_CONCURRENCY: usize = 8;

enum State {
    Queued { path: Vec<String>, args: Vec<Value> },
    Running(BoxFuture<'static, Eval<Value>>),
    Done(Eval<Value>),
}

struct Task {
    state: State,
    /// Set when a `Promise.race` this task lost interrupted (or tried to interrupt) it.
    race_interrupted: bool,
}

pub(crate) struct Pool {
    tools: Arc<ToolRuntime>,
    tasks: IndexMap<usize, Task>,
    queue: VecDeque<usize>,
    running: usize,
    /// Tool-call promises whose settlement no program construct has observed yet.
    pub unobserved: IndexSet<usize>,
}

impl Pool {
    pub fn new(tools: Arc<ToolRuntime>) -> Self {
        Self {
            tools,
            tasks: IndexMap::new(),
            queue: VecDeque::new(),
            running: 0,
            unobserved: IndexSet::new(),
        }
    }

    pub fn tools(&self) -> &Arc<ToolRuntime> {
        &self.tools
    }

    /// Creates a task for a tool call; it is admitted immediately when a permit is free.
    pub fn spawn(&mut self, path: Vec<String>, args: Vec<Value>) -> usize {
        let id = crate::value::next_id();
        self.tasks.insert(
            id,
            Task {
                state: State::Queued { path, args },
                race_interrupted: false,
            },
        );
        self.unobserved.insert(id);
        self.queue.push_back(id);
        self.start_queued();
        id
    }

    fn start_queued(&mut self) {
        while self.running < TOOL_CALL_CONCURRENCY {
            let Some(id) = self.queue.pop_front() else {
                return;
            };
            let Some(task) = self.tasks.get_mut(&id) else {
                continue;
            };
            let State::Queued { path, args } =
                std::mem::replace(&mut task.state, State::Done(Ok(Value::Undefined)))
            else {
                continue;
            };
            task.state = match self.tools.admit(&path, &args) {
                Ok(admitted) => {
                    self.running += 1;
                    State::Running(self.tools.clone().run(admitted).boxed())
                }
                Err(fault) => State::Done(Err(fault)),
            };
        }
    }

    /// Polls every running task once (in creation order), starting queued work as permits free.
    pub fn poll_all(&mut self, context: &mut Context<'_>) {
        loop {
            let mut finished = false;
            for task in self.tasks.values_mut() {
                if let State::Running(future) = &mut task.state
                    && let Poll::Ready(result) = future.poll_unpin(context)
                {
                    task.state = State::Done(result);
                    self.running -= 1;
                    finished = true;
                }
            }
            if !finished {
                return;
            }
            self.start_queued();
        }
    }

    /// Polls one task once so its host work starts at the call site.
    pub fn poll_one(&mut self, id: usize, context: &mut Context<'_>) {
        if let Some(task) = self.tasks.get_mut(&id)
            && let State::Running(future) = &mut task.state
            && let Poll::Ready(result) = future.poll_unpin(context)
        {
            task.state = State::Done(result);
            self.running -= 1;
            self.start_queued();
        }
    }

    /// The task's settlement once done (`None` while pending).
    pub fn result(&self, id: usize) -> Option<(Eval<Value>, bool)> {
        let task = self.tasks.get(&id)?;
        match &task.state {
            State::Done(result) => Some((result.clone(), task.race_interrupted)),
            _ => None,
        }
    }

    pub fn is_done(&self, id: usize) -> bool {
        self.tasks
            .get(&id)
            .is_none_or(|task| matches!(task.state, State::Done(_)))
    }

    /// Interrupts a race loser: drops its host future and settles it as interrupted.
    pub fn interrupt(&mut self, id: usize) {
        let Some(task) = self.tasks.get_mut(&id) else {
            return;
        };
        task.race_interrupted = true;
        match task.state {
            State::Running(_) => {
                task.state = State::Done(Err(Fault::Interrupted));
                self.running -= 1;
                self.start_queued();
            }
            State::Queued { .. } => {
                self.queue.retain(|queued| *queued != id);
                task.state = State::Done(Err(Fault::Interrupted));
            }
            State::Done(_) => {}
        }
    }

    /// Waits for a task's settlement while driving every other task.
    pub async fn wait(&mut self, id: usize) -> (Eval<Value>, bool) {
        futures::future::poll_fn(|context| {
            self.poll_all(context);
            match self.result(id) {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Waits until any of `ids` settles; returns the first settled id in list order.
    pub async fn wait_any(&mut self, ids: &[usize]) -> usize {
        futures::future::poll_fn(|context| {
            self.poll_all(context);
            match ids.iter().find(|id| self.is_done(**id)) {
                Some(id) => Poll::Ready(*id),
                None => Poll::Pending,
            }
        })
        .await
    }
}
