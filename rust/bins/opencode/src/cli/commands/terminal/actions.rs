//! Purpose: dispatch terminal navigation and session commands through real Process services.
//! Scope: one application action.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/terminal.rs
//! `-- opencode/src/cli/commands/terminal/permission.rs
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- ui-terminal/src/app.rs
//! `-- ui-terminal/src/dialog/mod.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: session lifecycle mutations use V2 services, never the separate legacy store.
//! Contract tests: opencode/tests/commands.rs terminal_app module

use super::{current, permission, projection, refresh};
use anyhow::Context;
use crossterm::terminal;
use opencode_runtime::Process;
use opencode_session_api::SessionID;
use opencode_session_api::operations::{
    ListRequest, ListScope, RevertStageRequest, SessionService,
};
use opencode_ui_terminal::app::{Action, App};
use opencode_ui_terminal::dialog::{Choice, Kind};
use opencode_ui_terminal::routes::Route;
use opencode_ui_terminal::timeline::Timeline;
use std::path::Path;

pub async fn dispatch(
    process: &Process,
    app: &mut App,
    timeline: &mut Timeline,
    directory: &Path,
    action: Action,
) -> anyhow::Result<()> {
    match action {
        Action::None | Action::Cancel | Action::Exit => {
            if app.session().is_none() {
                timeline.clear();
            }
        }
        Action::New => {
            app.navigation.navigate(Route::Home);
            timeline.clear();
            app.status = "New session".into();
        }
        Action::Open(id) => {
            let info = process.session.get(&SessionID::new(&id)?).await?;
            app.selected_variant = info
                .model
                .as_ref()
                .and_then(|model| model.variant.as_ref().map(ToString::to_string));
            app.selected_model = info
                .model
                .map(|model| format!("{}/{}", model.provider_id, model.id));
            app.selected_agent = info.agent.map(|agent| agent.to_string());
            app.open(id);
            refresh(process, app, timeline).await?;
        }
        Action::Sessions => {
            let sessions = process
                .session
                .list(ListRequest {
                    scope: ListScope::Directory(directory.to_string_lossy().into_owned()),
                    limit: Some(100),
                    ..ListRequest::default()
                })
                .await?;
            app.picker(
                Kind::Sessions,
                "Sessions",
                sessions
                    .into_iter()
                    .filter(|session| session.parent_id.is_none())
                    .map(|session| {
                        Choice::new(
                            session.id.to_string(),
                            session.title,
                            session.id.to_string(),
                        )
                    })
                    .collect(),
            );
        }
        Action::Models => {
            let models = process.available_models(directory.to_path_buf()).await?;
            app.picker(
                Kind::Models,
                "Select model",
                models
                    .into_iter()
                    .map(|model| {
                        Choice::new(
                            format!("{}/{}", model.provider_id, model.id),
                            model.name,
                            model.provider_id.to_string(),
                        )
                    })
                    .collect(),
            );
        }
        Action::Agents => {
            let agents = process.available_agents(directory.to_path_buf()).await?;
            app.picker(
                Kind::Agents,
                "Select agent",
                agents
                    .into_iter()
                    .filter(|agent| {
                        !agent.hidden
                            && serde_json::to_value(agent.mode).ok()
                                != Some(serde_json::json!("subagent"))
                    })
                    .map(|agent| {
                        Choice::new(
                            agent.id.to_string(),
                            agent.id.to_string(),
                            agent.description.unwrap_or_default(),
                        )
                    })
                    .collect(),
            );
        }
        Action::Variants => {
            let selected = match app.selected_model.clone() {
                Some(model) => model,
                None => process
                    .default_model(directory.to_path_buf())
                    .await?
                    .map(|model| format!("{}/{}", model.provider_id, model.id))
                    .context("Select a model first")?,
            };
            let model = process
                .available_models(directory.to_path_buf())
                .await?
                .into_iter()
                .find(|model| format!("{}/{}", model.provider_id, model.id) == selected)
                .context("Selected model is no longer available")?;
            let mut choices = vec![Choice::new("", "Default", "Provider default")];
            choices.extend(
                model
                    .variants
                    .into_iter()
                    .map(|variant| Choice::new(variant.id.to_string(), variant.id.to_string(), "")),
            );
            app.selected_model = Some(selected);
            app.picker(Kind::Variants, "Select variant", choices);
        }
        Action::Variant(value) => {
            let variant = (!value.is_empty()).then_some(value);
            if app.session().is_some() {
                let mut model = process
                    .session
                    .get(&current(app)?)
                    .await?
                    .model
                    .or(process.default_model(directory.to_path_buf()).await?)
                    .context("Select a model first")?;
                model.variant = variant
                    .as_ref()
                    .map(|variant| serde_json::from_value(serde_json::json!(variant)))
                    .transpose()?;
                process.session.switch_model(&current(app)?, model).await?;
            }
            app.selected_variant = variant;
            app.status = format!(
                "Variant: {}",
                app.selected_variant.as_deref().unwrap_or("Default")
            );
        }
        Action::Model(value) => {
            let (provider, model) = value.split_once('/').context("Invalid model reference")?;
            if app.session().is_some() {
                process
                    .session
                    .switch_model(
                        &current(app)?,
                        serde_json::from_value(
                            serde_json::json!({"providerID":provider,"id":model}),
                        )?,
                    )
                    .await?;
            }
            app.selected_model = Some(value.clone());
            app.selected_variant = None;
            app.status = format!("Model: {value}");
        }
        Action::Agent(value) => {
            if app.session().is_some() {
                process.session.switch_agent(&current(app)?, &value).await?;
            }
            app.selected_agent = Some(value.clone());
            app.status = format!("Agent: {value}");
        }
        Action::Timeline => {
            let messages = process.session.context(&current(app)?).await?;
            app.picker(
                Kind::Timeline,
                "Jump to message",
                messages
                    .into_iter()
                    .rev()
                    .filter(|message| {
                        matches!(message, opencode_session_api::message::Message::User(_))
                    })
                    .map(|message| -> anyhow::Result<Choice> {
                        let id = message.id().to_string();
                        let value = serde_json::to_value(message)?;
                        Ok(Choice::new(
                            id,
                            value["text"].as_str().unwrap_or("Assistant response"),
                            value["type"].as_str().unwrap_or_default(),
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            );
        }
        Action::Jump(id) => {
            let messages = process.session.context(&current(app)?).await?;
            let index = messages
                .iter()
                .position(|message| message.id().as_str() == id)
                .context("Message no longer exists")?;
            let mut after = Timeline::default();
            projection::project(&messages[index..], app.thinking, &mut after)?;
            let (width, _) = terminal::size()?;
            app.scroll = after.lines(width).len().saturating_sub(2);
        }
        Action::Undo => {
            let session_id = current(app)?;
            let info = process.session.get(&session_id).await?;
            let messages = process.session.context(&session_id).await?;
            let message = messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    opencode_session_api::message::Message::User(user)
                        if info
                            .revert
                            .as_ref()
                            .is_none_or(|revert| user.id != revert.message_id) =>
                    {
                        Some(user)
                    }
                    _ => None,
                })
                .context("No user message to undo")?;
            process
                .session
                .revert_stage(RevertStageRequest {
                    session_id,
                    message_id: message.id.clone(),
                    files: Some(true),
                })
                .await?;
            app.pending_prompt = Some(message.text.clone());
            refresh(process, app, timeline).await?;
            app.status = "Undo staged; /redo restores the message".into();
        }
        Action::Redo => {
            process.session.revert_clear(&current(app)?).await?;
            refresh(process, app, timeline).await?;
        }
        Action::Compact => {
            process.session.compact(&current(app)?).await?;
            refresh(process, app, timeline).await?;
        }
        Action::Rename(title) => {
            process.session.rename(&current(app)?, &title).await?;
            app.status = format!("Renamed session: {title}");
        }
        Action::Delete => {
            let id = current(app)?;
            process.session.delete(&id).await?;
            app.navigation.forget(id.as_str());
            timeline.clear();
            app.status = "Session deleted".into();
        }
        Action::Fork => {
            let messages = process.session.context(&current(app)?).await?;
            let mut choices = vec![Choice::new("", "Full session", "All visible history")];
            choices.extend(
                messages
                    .into_iter()
                    .rev()
                    .filter_map(|message| match message {
                        opencode_session_api::message::Message::User(user) => Some(Choice::new(
                            user.id.to_string(),
                            user.text.replace('\n', " "),
                            "Fork before this prompt",
                        )),
                        _ => None,
                    }),
            );
            app.picker(Kind::Fork, "Fork session", choices);
        }
        Action::ForkAt(id) => {
            let message_id = (!id.is_empty())
                .then(|| opencode_session_api::MessageID::new(&id))
                .transpose()?;
            let prompt = match &message_id {
                Some(id) => {
                    process
                        .session
                        .message(&current(app)?, id)
                        .await?
                        .and_then(|message| match message {
                            opencode_session_api::message::Message::User(user) => Some(user.text),
                            _ => None,
                        })
                }
                None => None,
            };
            let fork = process
                .session
                .fork(&current(app)?, message_id.as_ref())
                .await?;
            app.open(fork.id.to_string());
            app.pending_prompt = prompt;
            refresh(process, app, timeline).await?;
            app.status = "Session forked".into();
        }
        Action::Permission(value) => permission::reply(process, app, directory, &value).await?,
    }
    Ok(())
}
