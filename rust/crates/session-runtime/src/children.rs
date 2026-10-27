//! Purpose: child/subagent Session lifecycle behind the `task` tool, porting
//! packages/opencode/src/tool/task.ts:98-371 (depth gate, permission inheritance, child creation or
//! resume, model/variant inheritance, foreground join, background start/extend, completion notification
//! to the parent) and the cancel hierarchy of packages/opencode/src/session/run-state.ts:111-145, plus
//! packages/opencode/src/agent/subagent-permissions.ts.
//! Scope: Location-scoped orchestration over a `ChildHost` port (`ComposedChildHost`: SQLite legacy
//! Session reads/creates plus the `ChildOps` seams wired by phase 91) and the process-local
//! `BackgroundJobs` registry.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/children/store.rs
//! `-- session-runtime/src/children/types.rs
//! External:
//! |-- futures::FutureExt
//! |-- futures::future::BoxFuture
//! |-- futures::future::join_all
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- tokio::select
//! |-- tokio_util::sync::CancellationToken
//! `-- tokio_util::task::TaskTracker
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/tests/children.rs
//! |-- session-runtime/tests/children/fixture.rs
//! `-- session-runtime/tests/children_background.rs
//! Invariants: the depth gate counts ancestors of the calling Session and fails before any permission
//! ask or child creation. A child job ID is the child Session ID. Foreground runs join the job and are
//! cancelled (child prompt cancel + job cancel) when the caller aborts; a promotion returns the
//! background result instead. Background completion injects exactly one synthetic parent prompt for
//! completed/error, none for cancelled. Cancelling a Session cancels every running job whose id,
//! `metadata.sessionId` or `metadata.parentSessionId` is in the transitive set, so no child outlives
//! its cancelled ancestor. Registry shutdown while joining a foreground run cancels the child prompt.
//! Contract tests: session-runtime/tests/children.rs, session-runtime/tests/children_background.rs

mod store;
mod types;

use std::collections::HashSet;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::background::{BackgroundJobs, JobInfo, JobStatus, StartInput};

pub use store::{
    ChildLocation, ChildOps, ComposedChildHost, LegacySessions, UNWIRED_AGENT, UNWIRED_PERMISSION,
    UNWIRED_PROMPT, UnwiredChildOps,
};
pub use types::{
    AgentInfo, CallingModel, ChildError, ChildHost, ChildReply, ModelSel, PermissionRule,
    SessionNode, TaskInput, TaskOutput, TaskRun, TaskState, derive_subagent_permission,
    render_output,
};

pub const TASK_TOOL: &str = "task";

pub const BACKGROUND_STARTED: &str = "The task is working in the background. You will be notified automatically when it finishes.\nDO NOT sleep, poll for progress, ask the task for status, or duplicate this task's work — avoid working with the same files or topics it is using.\nWork on non-overlapping tasks, or briefly tell the user what you launched and end your response.";
pub const BACKGROUND_UPDATED: &str = "Additional context sent to the running background task.\nThe task is still working in the background. You will be notified automatically when it finishes.\nDO NOT sleep, poll for progress, ask the task for status, or duplicate this task's work — avoid working with the same files or topics it is using.\nWork on non-overlapping tasks, or briefly tell the user what you sent and end your response.";

/// Settings read from config and runtime flags by the caller.
#[derive(Clone, Debug, Default)]
pub struct TaskSettings {
    /// `cfg.subagent_depth`, default 1.
    pub subagent_depth: Option<u32>,
    /// `cfg.experimental.primary_tools`.
    pub primary_tools: Vec<String>,
    /// `OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS`.
    pub background_subagents: bool,
}

/// Task tool orchestration for one Location.
#[derive(Clone)]
pub struct Children {
    host: Arc<dyn ChildHost>,
    jobs: BackgroundJobs,
    /// Notification and injection tasks (`Effect.forkIn(scope)`), joined by `shutdown`.
    tasks: TaskTracker,
}

impl Children {
    pub fn new(host: Arc<dyn ChildHost>, jobs: BackgroundJobs) -> Self {
        Self {
            host,
            jobs,
            tasks: TaskTracker::new(),
        }
    }

    pub fn jobs(&self) -> &BackgroundJobs {
        &self.jobs
    }

    /// `TaskTool.execute`. `abort` is the tool call abort signal of the parent turn.
    pub async fn run(
        &self,
        settings: &TaskSettings,
        input: TaskInput,
        abort: CancellationToken,
    ) -> Result<TaskOutput, ChildError> {
        let background = input.background == Some(true);
        if background && !settings.background_subagents {
            return Err(ChildError::Failed(
                "Background subagents require OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true"
                    .into(),
            ));
        }
        let parent = self.host.session(&input.session_id).await?;
        let limit = settings.subagent_depth.unwrap_or(1);
        if self.depth(&parent).await? >= limit {
            return Err(ChildError::Failed(format!(
                "Subagent depth limit reached ({limit}). Increase \"subagent_depth\" to allow nested subagents."
            )));
        }
        if !input.bypass_agent_check {
            self.host
                .ask_permission(&input, TASK_TOOL, &input.subagent_type)
                .await?;
        }
        let agent = self
            .host
            .agent(&input.subagent_type)
            .await?
            .ok_or_else(|| {
                ChildError::Failed(format!(
                    "Unknown agent type: {} is not a valid agent type",
                    input.subagent_type
                ))
            })?;
        let existing = match &input.task_id {
            Some(id) => self.host.session(id).await.ok(),
            None => None,
        };
        let child = match existing {
            Some(child) => child,
            None => {
                let permission = child_permission(&parent, &agent, &settings.primary_tools);
                let title = format!("{} (@{} subagent)", input.description, agent.name);
                self.host
                    .create_child(&input.session_id, title, &agent.name, permission)
                    .await?
            }
        };
        let calling = self.host.calling_model(&input).await?;
        let model = agent.model.clone().unwrap_or_else(|| calling.model.clone());
        let mut metadata = Map::new();
        metadata.insert("parentSessionId".into(), input.session_id.clone().into());
        metadata.insert("sessionId".into(), child.id.clone().into());
        // Key order follows the baseline objects: `Provider.parseModel` for agent models,
        // `{ modelID, providerID }` for the calling message model.
        let model_json = match agent.model.is_some() {
            true => serde_json::json!({"providerID": model.provider_id, "modelID": model.model_id}),
            false => {
                serde_json::json!({"modelID": model.model_id, "providerID": model.provider_id})
            }
        };
        metadata.insert("model".into(), model_json);
        if background {
            metadata.insert("background".into(), Value::Bool(true));
        }
        self.host
            .tool_metadata(&input, &input.description, &metadata)
            .await;
        self.host.prompt_ready()?;
        let run = TaskRun {
            child: child.id.clone(),
            model: model.clone(),
            variant: if agent.model.is_some() {
                None
            } else {
                calling.variant.clone()
            },
            agent: agent.name.clone(),
            prompt: input.prompt.clone(),
        };
        let background_meta = |job: &str| {
            let mut meta = metadata.clone();
            meta.insert("background".into(), Value::Bool(true));
            meta.insert("jobId".into(), job.into());
            meta
        };
        let running = |summary: &str, text: &str, job: &str| TaskOutput {
            title: input.description.clone(),
            metadata: background_meta(job),
            output: render_output(&child.id, TaskState::Running, Some(summary), text),
        };

        if self.jobs.extend(&child.id, self.child_run(run.clone())) {
            return Ok(running(
                "Background task updated",
                BACKGROUND_UPDATED,
                &child.id,
            ));
        }

        let notify = self.notifier(&input, calling.variant.clone(), &child.id);
        let on_promote = {
            let host = self.host.clone();
            let (input, meta) = (input.clone(), background_meta(&child.id));
            let notify = notify.clone();
            async move {
                host.tool_metadata(&input, &input.description, &meta).await;
                notify();
            }
            .boxed()
        };
        let on_interrupt = {
            let (host, child) = (self.host.clone(), child.id.clone());
            async move { host.cancel(&child).await }.boxed()
        };
        let info = self.jobs.start(StartInput {
            id: Some(child.id.clone()),
            job_type: TASK_TOOL.into(),
            title: Some(input.description.clone()),
            metadata: Some(metadata.clone()),
            on_promote: Some(on_promote),
            on_interrupt: Some(on_interrupt),
            run: self.child_run(run),
        });

        if background {
            notify();
            return Ok(running(
                "Background task started",
                BACKGROUND_STARTED,
                &info.id,
            ));
        }

        let joined = tokio::select! {
            waited = self.jobs.wait(&child.id, None) => Some(waited.info),
            promoted = self.jobs.wait_for_promotion(&child.id) => Some(Some(promoted)),
            _ = abort.cancelled() => None,
        };
        let Some(result) = joined else {
            self.host.cancel(&child.id).await;
            self.jobs.cancel(&child.id).await;
            return Err(ChildError::Cancelled);
        };
        // `None`: the owning scope shut the registry down, which interrupts the baseline tool fiber.
        let Some(result) = result else {
            self.host.cancel(&child.id).await;
            return Err(ChildError::Cancelled);
        };
        match result {
            info if info.is_background() => Ok(running(
                "Background task started",
                BACKGROUND_STARTED,
                &info.id,
            )),
            JobInfo {
                status: JobStatus::Error,
                error,
                ..
            } => Err(ChildError::Failed(
                error.unwrap_or_else(|| "Task failed".into()),
            )),
            JobInfo {
                status: JobStatus::Cancelled,
                ..
            } => Err(ChildError::Failed("Task cancelled".into())),
            info => Ok(TaskOutput {
                title: input.description.clone(),
                metadata,
                output: render_output(
                    &child.id,
                    TaskState::Completed,
                    None,
                    info.output.as_deref().unwrap_or(""),
                ),
            }),
        }
    }

    /// `SessionRunState.cancelBackgroundJobs`: cancel the Session's jobs and every descendant job.
    pub async fn cancel_tree(&self, session_id: &str) {
        let jobs = self.jobs.list();
        let mut pending: HashSet<String> = HashSet::from([session_id.to_owned()]);
        let mut cancelled: HashSet<String> = HashSet::new();
        loop {
            let batch: Vec<&JobInfo> = jobs
                .iter()
                .filter(|job| job.status == JobStatus::Running && !cancelled.contains(&job.id))
                .filter(|job| {
                    pending.contains(&job.id)
                        || job
                            .metadata_str("sessionId")
                            .is_some_and(|id| pending.contains(id))
                        || job
                            .metadata_str("parentSessionId")
                            .is_some_and(|id| pending.contains(id))
                })
                .collect();
            if batch.is_empty() {
                return;
            }
            futures::future::join_all(batch.iter().map(|job| self.jobs.cancel(&job.id))).await;
            batch.into_iter().for_each(|job| {
                cancelled.insert(job.id.clone());
                pending.insert(job.id.clone());
                if let Some(id) = job.metadata_str("sessionId") {
                    pending.insert(id.to_owned());
                }
            });
        }
    }

    /// Join notification/injection tasks after the owning scope stops the jobs.
    pub async fn shutdown(&self) {
        self.jobs.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
    }

    async fn depth(&self, parent: &SessionNode) -> Result<u32, ChildError> {
        let mut depth = 0;
        let mut current = parent.parent_id.clone();
        while let Some(id) = current {
            depth += 1;
            current = self.host.session(&id).await?.parent_id;
        }
        Ok(depth)
    }

    fn child_run(&self, run: TaskRun) -> BoxFuture<'static, Result<String, String>> {
        let host = self.host.clone();
        let scope = host.scope();
        async move {
            let _scope = scope;
            host.prompt_child(run)
                .await
                .map_err(|error| error.to_string())
        }
        .boxed()
    }

    /// `notifyBackgroundResult`: wait for the job and inject one synthetic parent prompt.
    fn notifier(
        &self,
        input: &TaskInput,
        variant: Option<String>,
        job: &str,
    ) -> Arc<dyn Fn() + Send + Sync> {
        let (host, jobs, tasks) = (self.host.clone(), self.jobs.clone(), self.tasks.clone());
        let scope = host.scope();
        let (input, job) = (input.clone(), job.to_owned());
        Arc::new(move || {
            let scope = scope.clone();
            let (host, jobs, input, job, variant) = (
                host.clone(),
                jobs.clone(),
                input.clone(),
                job.clone(),
                variant.clone(),
            );
            tasks.spawn(async move {
                let _scope = scope;
                let waited = jobs.wait(&job, None).await;
                let Some(info) = waited.info else { return };
                let (state, text) = match info.status {
                    JobStatus::Completed => (TaskState::Completed, info.output.unwrap_or_default()),
                    JobStatus::Error => (TaskState::Error, info.error.unwrap_or_default()),
                    _ => return,
                };
                let summary = match state {
                    TaskState::Completed => {
                        format!("Background task completed: {}", input.description)
                    }
                    _ => format!("Background task failed: {}", input.description),
                };
                let text = render_output(&job, state, Some(&summary), &text);
                // Failures are ignored like `Effect.ignore` in the baseline.
                let _ = host.inject_parent(&input, variant, text).await;
            });
        })
    }
}

/// Child permission: derived rules plus tool denies not already present.
fn child_permission(
    parent: &SessionNode,
    agent: &AgentInfo,
    primary_tools: &[String],
) -> Vec<PermissionRule> {
    let derived = derive_subagent_permission(&parent.permission, agent);
    let has = |permission: &str| {
        agent
            .permission
            .iter()
            .any(|rule| rule.permission == permission)
    };
    let denies = ["todowrite", TASK_TOOL]
        .into_iter()
        .filter(|permission| !has(permission))
        .map(str::to_owned)
        .chain(primary_tools.iter().cloned())
        .map(|permission| PermissionRule::deny(&permission));
    let extra: Vec<_> = denies.filter(|deny| !derived.contains(deny)).collect();
    derived.into_iter().chain(extra).collect()
}
