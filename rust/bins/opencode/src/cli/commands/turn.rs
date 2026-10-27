//! Purpose: stream projected assistant output while joining a real process-owned Session drain.
//! Scope: one admitted CLI prompt.
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/message.rs
//! `-- session-api/src/operations.rs
//! External:
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- tokio::pin
//! |-- tokio::select
//! `-- tokio::time::interval
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: previous responses are not reprinted; failures and cancellation never become success.
//! Contract tests: opencode/tests/commands.rs

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::bail;
use opencode_runtime::{Process, Reply, ReplyInput};
use opencode_session_api::admission::PromptRequest;
use opencode_session_api::message::{AssistantContent, Message};
use opencode_session_api::operations::{SessionExecution, SessionService};
use opencode_session_api::{PromptInput, SessionID};

pub async fn execute(
    process: &Process,
    session: &SessionID,
    text: String,
    thinking: bool,
    mut output: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let directory = std::path::PathBuf::from(
        process
            .session
            .get(session)
            .await?
            .location
            .directory
            .as_str(),
    );
    let previous: BTreeSet<_> = process
        .session
        .context(session)
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
            delivery: None,
            resume: Some(false),
        })
        .await?;
    let drain = process.execution.resume(session);
    tokio::pin!(drain);
    let mut interval = tokio::time::interval(Duration::from_millis(30));
    let mut printed = BTreeMap::new();
    let result = loop {
        tokio::select! {
            result = &mut drain => break result.map_err(anyhow::Error::from),
            result = tokio::signal::ctrl_c() => {
                break match result {
                    Ok(()) => Err(anyhow::anyhow!("Session interrupted")),
                    Err(error) => Err(error.into()),
                };
            }
            _ = interval.tick() => {
                let refreshed = async {
                    reject_pending(process, session, &directory).await?;
                    render(process, session, &previous, &mut printed, thinking, &mut output).await
                }.await;
                if let Err(error) = refreshed {
                    break Err(error);
                }
            }
        }
    };
    if result.is_err() {
        process.execution.interrupt(session).await;
        return result;
    }
    render(
        process,
        session,
        &previous,
        &mut printed,
        thinking,
        &mut output,
    )
    .await?;
    let messages = process.session.context(session).await?;
    let assistants: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) if !previous.contains(&assistant.id) => Some(assistant),
            _ => None,
        })
        .collect();
    if assistants.is_empty() {
        bail!("Session drain produced no assistant response");
    }
    for assistant in assistants {
        if let Some(error) = &assistant.error {
            bail!("{}", error.message);
        }
        if assistant.time.completed.is_none() {
            bail!("Session drain left an incomplete assistant response");
        }
    }
    Ok(())
}

async fn reject_pending(
    process: &Process,
    session: &SessionID,
    directory: &std::path::Path,
) -> anyhow::Result<()> {
    for request in process.pending_permissions(directory.to_path_buf()).await? {
        if request.session_id != session.as_str() {
            continue;
        }
        eprintln!(
            "permission requested: {} ({}); auto-rejecting",
            request.action,
            request.resources.join(", ")
        );
        process
            .reply_permission(
                directory.to_path_buf(),
                ReplyInput {
                    request_id: request.id,
                    reply: Reply::Reject,
                    message: None,
                },
            )
            .await?;
    }
    for request in process.pending_questions(directory.to_path_buf()).await? {
        if &request.session_id != session {
            continue;
        }
        process
            .reject_question(directory.to_path_buf(), request.id.as_str())
            .await?;
    }
    Ok(())
}

async fn render(
    process: &Process,
    session: &SessionID,
    previous: &BTreeSet<opencode_session_api::MessageID>,
    printed: &mut BTreeMap<String, usize>,
    thinking: bool,
    output: &mut impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for message in process.session.context(session).await? {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        if previous.contains(&assistant.id) {
            continue;
        }
        for part in assistant.content {
            let (id, text) = match part {
                AssistantContent::Text(part) => (part.id, part.text),
                AssistantContent::Reasoning(part) if thinking => (part.id, part.text),
                _ => continue,
            };
            let offset = printed
                .entry(format!("{}/{}", assistant.id, id))
                .or_insert(0);
            if let Some(delta) = text.get(*offset..)
                && !delta.is_empty()
            {
                output(delta)?;
            }
            *offset = text.len();
        }
    }
    Ok(())
}
