//! Purpose: own and join process execution tasks while the UI navigates between sessions.
//! Scope: one terminal application lifetime.
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/message.rs
//! `-- session-api/src/operations.rs
//! External:
//! `-- tokio::task::JoinHandle
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: admitted inputs precede execution, every spawned join handle is joined on exit.
//! Contract tests: opencode/tests/commands.rs terminal_app and pty modules

use opencode_runtime::Process;
use opencode_session_api::admission::{Delivery, PromptRequest};
use opencode_session_api::message::Message;
use opencode_session_api::operations::{SessionExecution, SessionService};
use opencode_session_api::{MessageID, PromptInput, SessionID};
use std::collections::{BTreeMap, BTreeSet};
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct Executions(BTreeMap<SessionID, Running>);
struct Running {
    previous: BTreeSet<MessageID>,
    task: JoinHandle<opencode_session_api::SessionResult<()>>,
}

impl Executions {
    pub fn active(&self, session: &SessionID) -> bool {
        self.0.contains_key(session)
    }

    pub async fn submit(
        &mut self,
        process: &Process,
        session: SessionID,
        text: String,
        delivery: Delivery,
    ) -> anyhow::Result<()> {
        let previous = process
            .session
            .context(&session)
            .await?
            .into_iter()
            .map(|message| message.id().clone())
            .collect();
        process
            .session
            .prompt(PromptRequest {
                id: None,
                session_id: session.clone(),
                prompt: PromptInput::text(text),
                delivery: Some(delivery),
                resume: Some(false),
            })
            .await?;
        if self.active(&session) {
            // Admission can race the active drain's final idle boundary. The advisory
            // wake closes that gap without inventing a second UI execution loop.
            process.execution.wake(&session).await;
            return Ok(());
        }
        let execution = process.execution.clone();
        let id = session.clone();
        self.0.insert(
            session,
            Running {
                previous,
                task: tokio::spawn(async move { execution.resume(&id).await }),
            },
        );
        Ok(())
    }

    pub async fn reap(&mut self, process: &Process) -> anyhow::Result<()> {
        let active = process.execution.active().await;
        let finished = self
            .0
            .iter()
            .filter(|(id, running)| running.task.is_finished() && !active.contains(*id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in finished {
            let Some(running) = self.0.remove(&id) else {
                continue;
            };
            running.task.await??;
            let messages = process.session.context(&id).await?;
            let assistants = messages
                .iter()
                .filter_map(|message| match message {
                    Message::Assistant(assistant) if !running.previous.contains(&assistant.id) => {
                        Some(assistant)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if assistants.is_empty() {
                anyhow::bail!("Session drain produced no assistant response");
            }
            for assistant in assistants {
                if let Some(error) = &assistant.error {
                    anyhow::bail!("{}", error.message);
                }
                if assistant.time.completed.is_none() {
                    anyhow::bail!("Session drain left an incomplete assistant response");
                }
            }
        }
        Ok(())
    }

    pub async fn cancel(&mut self, process: &Process, session: &SessionID) {
        process.execution.interrupt(session).await;
        if let Some(running) = self.0.remove(session) {
            let _ = running.task.await;
        }
    }

    pub async fn shutdown(&mut self, process: &Process) {
        for (id, running) in std::mem::take(&mut self.0) {
            process.execution.interrupt(&id).await;
            let _ = running.task.await;
        }
    }
}
