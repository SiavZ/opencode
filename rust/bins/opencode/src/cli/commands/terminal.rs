//! Purpose: run the full-screen application over process-owned services while execution continues across navigation.
//! Scope: terminal application lifetime.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/commands/terminal/actions.rs
//! |-- opencode/src/cli/commands/terminal/execution.rs
//! |-- opencode/src/cli/commands/terminal/permission.rs
//! |-- opencode/src/cli/commands/terminal/projection.rs
//! `-- opencode/src/cli/spec/mod.rs
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- session-api/src/operations.rs
//! |-- ui-terminal/src/app.rs
//! |-- ui-terminal/src/prompt/mod.rs
//! |-- ui-terminal/src/rendering.rs
//! `-- ui-terminal/src/terminal.rs
//! External:
//! |-- crossterm::{event, terminal}
//! `-- tokio::time
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/mod.rs
//! Invariants: every execution task is joined; terminal restoration happens on success, error and cancellation.
//! Contract tests: opencode/tests/commands.rs terminal_app and pty modules

mod actions;
mod execution;
mod history;
mod permission;
mod projection;
mod question;

use super::super::spec::{CommandId, Parsed};
use actions::dispatch;
use anyhow::{Context, bail};
use crossterm::{event, terminal};
use execution::Executions;
use opencode_runtime::Process;
use opencode_session_api::SessionID;
use opencode_session_api::admission::Delivery;
use opencode_session_api::operations::{CreateRequest, SessionService};
use opencode_ui_terminal::app::{Action, App};
use opencode_ui_terminal::input::UiEvent;
use opencode_ui_terminal::prompt::{Editor, EditorAction};
use opencode_ui_terminal::rendering::{FrameInput, Renderer};
use opencode_ui_terminal::terminal::{TerminalGuard, TerminalOptions};
use opencode_ui_terminal::timeline::Timeline;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub async fn run(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("TUI requires a TTY stdin and stdout; use `opencode run` for piped input");
    }
    let initial = if parsed.command == CommandId::Tui {
        parsed.args.str("prompt").unwrap_or_default()
    } else {
        parsed
            .args
            .strings("message")
            .into_iter()
            .chain(parsed.double_dash.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut app = App::default();
    app.thinking = parsed.args.bool("thinking");
    let mut editor = Editor::new();
    let mut history =
        history::History::load(process.options.paths.state.join("prompt-history.jsonl")).await;
    editor.set_history(history.inputs());
    let mut drafts = BTreeMap::new();
    let mut editor_route = app.session().unwrap_or_default().to_owned();
    let mut timeline = Timeline::default();
    let mut renderer = Renderer::default();
    let mut executions = Executions::default();
    let mut question = None;
    if parsed.args.str("session").is_some()
        || parsed.args.bool("continue")
        || !initial.trim().is_empty()
        || parsed.args.str("model").is_some()
        || parsed.args.str("agent").is_some()
    {
        app.open(
            super::session(process, parsed, directory.clone())
                .await?
                .to_string(),
        );
        if parsed.args.bool_opt("replay") != Some(false) && !parsed.args.bool("no-replay") {
            refresh(process, &app, &mut timeline).await?;
        } else {
            app.hidden_messages = process
                .session
                .context(&current(&app)?)
                .await?
                .iter()
                .map(|message| message.id().to_string())
                .collect();
        }
    }
    let guard = TerminalGuard::with_options(TerminalOptions {
        mouse: false,
        kitty_keyboard: false,
    })?;
    let result = async {
        if !initial.trim().is_empty() {
            let _ = history.append(&initial).await;
            executions
                .submit(process, current(&app)?, initial, Delivery::Steer)
                .await?;
        }
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        loop {
            interval.tick().await;
            if guard.termination_signal().is_some() {
                break;
            }
            executions.reap(process).await?;
            let route = app.session().unwrap_or_default().to_owned();
            if route != editor_route {
                drafts.insert(editor_route, editor.clone());
                editor = drafts.remove(&route).unwrap_or_default();
                editor.set_history(history.inputs());
                editor_route = route;
                question = None;
            }
            if let Some(text) = app.pending_prompt.take() {
                editor.set_text(text);
            }
            let busy = app
                .session()
                .map(SessionID::new)
                .transpose()?
                .is_some_and(|id| executions.active(&id));
            if app.busy && !busy {
                app.status = "Enter sends | Ctrl-P commands | Ctrl-D exits".into();
            }
            app.busy = busy;
            if busy {
                let id = current(&app)?;
                let location = process.session.get(&id).await?.location.directory;
                permission::poll(process, &mut app, Path::new(location.as_str()), id.as_str())
                    .await?;
                question::poll(
                    process,
                    &mut question,
                    Path::new(location.as_str()),
                    id.as_str(),
                )
                .await?;
                if app.dialogs.top().is_none() {
                    app.status = "Running | Esc cancels | Enter steers | /queue waits".into();
                }
            }
            refresh(process, &app, &mut timeline).await?;
            draw(&app, &editor, &timeline, &mut renderer, question.as_ref())?;
            if !event::poll(Duration::ZERO)? {
                continue;
            }
            let event = UiEvent::from(event::read()?);
            if question.is_some() && app.dialogs.top().is_none() {
                let location = process
                    .session
                    .get(&current(&app)?)
                    .await?
                    .location
                    .directory;
                if let Err(error) = question::handle(
                    process,
                    &mut question,
                    Path::new(location.as_str()),
                    &event,
                    terminal::size()?.0.saturating_sub(2),
                )
                .await
                {
                    app.error(error);
                }
                continue;
            }
            if let Some(action) = app.handle(&event) {
                match action {
                    Action::Exit => break,
                    Action::Cancel => {
                        executions.cancel(process, &current(&app)?).await;
                        bail!("Session interrupted");
                    }
                    action => {
                        if matches!(action, Action::Delete | Action::Undo) && app.busy {
                            executions.cancel(process, &current(&app)?).await;
                        }
                        if let Err(error) =
                            dispatch(process, &mut app, &mut timeline, &directory, action).await
                        {
                            app.error(error);
                        }
                    }
                }
                continue;
            }
            let EditorAction::Submit(text) =
                editor.handle_at(&event, terminal::size()?.0.saturating_sub(2))
            else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            let queued = text.strip_prefix("/queue ").map(str::to_owned);
            if text.starts_with('/') && queued.is_none() {
                let action = app.command(text.trim());
                if action == Action::Exit {
                    break;
                }
                if let Err(error) =
                    dispatch(process, &mut app, &mut timeline, &directory, action).await
                {
                    app.error(error);
                }
                continue;
            }
            if app.session().is_none() {
                new_session(process, &mut app, &directory).await?;
            }
            let delivery = if queued.is_some() {
                Delivery::Queue
            } else {
                Delivery::Steer
            };
            let _ = history.append(&text).await;
            executions
                .submit(process, current(&app)?, queued.unwrap_or(text), delivery)
                .await?;
            app.scroll = 0;
        }
        Ok(())
    }
    .await;
    executions.shutdown(process).await;
    guard.restore()?;
    result
}

async fn new_session(process: &Process, app: &mut App, directory: &Path) -> anyhow::Result<()> {
    let previous = match app.session() {
        Some(id) => Some(process.session.get(&SessionID::new(id)?).await?),
        None => None,
    };
    let session = process
        .session
        .create(CreateRequest {
            id: None,
            agent: app
                .selected_agent
                .as_ref()
                .map(opencode_session_api::AgentID::new)
                .or_else(|| previous.as_ref().and_then(|info| info.agent.clone())),
            model: match app.selected_model.as_ref() {
                Some(model) => {
                    let (provider, id) =
                        model.split_once('/').context("Invalid model reference")?;
                    Some(opencode_session_api::ModelRef {
                        provider_id: opencode_session_api::ids::ProviderID::new(provider),
                        id: opencode_session_api::ids::ModelID::new(id),
                        variant: app
                            .selected_variant
                            .as_ref()
                            .map(|variant| serde_json::from_value(serde_json::json!(variant)))
                            .transpose()?,
                    })
                }
                None => previous.as_ref().and_then(|info| info.model.clone()),
            },
            location: serde_json::from_value(
                serde_json::json!({"directory":directory.to_string_lossy()}),
            )?,
        })
        .await?;
    app.open(session.id.to_string());
    app.status = "New session".into();
    Ok(())
}

fn current(app: &App) -> anyhow::Result<SessionID> {
    Ok(SessionID::new(
        app.session().context("Open a session first")?,
    )?)
}

async fn refresh(process: &Process, app: &App, timeline: &mut Timeline) -> anyhow::Result<()> {
    if let Some(id) = app.session() {
        projection::project(
            &process
                .session
                .context(&SessionID::new(id)?)
                .await?
                .into_iter()
                .filter(|message| {
                    !app.hidden_messages
                        .iter()
                        .any(|id| id == message.id().as_str())
                })
                .collect::<Vec<_>>(),
            app.thinking,
            timeline,
        )?;
    }
    if app.session().is_none() {
        timeline.clear();
    }
    Ok(())
}

fn draw(
    app: &App,
    editor: &Editor,
    timeline: &Timeline,
    renderer: &mut Renderer,
    question: Option<&question::Form>,
) -> anyhow::Result<()> {
    let (width, height) = terminal::size()?;
    let overlay = app
        .dialogs
        .top()
        .map(|dialog| dialog.lines(usize::from(height.saturating_sub(7))))
        .or_else(|| question.map(question::Form::lines))
        .unwrap_or_default();
    let header = app
        .session()
        .map(|id| format!("OpenCode | Session {id}"))
        .unwrap_or_else(|| "OpenCode | Home | Start a conversation".into());
    let status = if app.status.is_empty() {
        "Enter sends | Ctrl-P commands | Ctrl-D exits"
    } else {
        &app.status
    };
    let output = renderer.render(&FrameInput {
        width,
        height,
        editor,
        timeline,
        header: &header,
        status,
        overlay: &overlay,
        scroll: app.scroll,
    })?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
