//! Purpose: basic full-screen local chat using the real terminal guard and input normalization.
//! Scope: terminal and Session lifetime, not full TypeScript TUI parity.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/mod.rs
//! |-- opencode/src/cli/commands/turn.rs
//! `-- opencode/src/cli/spec/mod.rs
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/message.rs
//! |-- session-api/src/operations.rs
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/terminal.rs
//! External:
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- crossterm::cursor
//! |-- crossterm::event
//! |-- crossterm::execute
//! |-- crossterm::terminal
//! |-- event::poll
//! |-- event::read
//! |-- terminal::size
//! |-- tokio::pin
//! |-- tokio::select
//! |-- tokio::time::interval
//! |-- turn::execute
//! `-- usize::from
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/mod.rs
//! Invariants: terminal modes restore on every exit; all turns use the process execution owner.
//! Contract tests: opencode/tests/commands.rs

use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::bail;
use crossterm::{cursor, event, execute, terminal};
use opencode_runtime::Process;
use opencode_session_api::SessionID;
use opencode_session_api::message::{AssistantContent, Message};
use opencode_session_api::operations::{SessionExecution, SessionService};
use opencode_ui_terminal::input::{UiEvent, UiKeyCode, UiKeyEventKind};
use opencode_ui_terminal::terminal::{TerminalGuard, TerminalOptions};

use super::super::spec::{CommandId, Parsed};
use super::turn;

pub async fn run(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("TUI requires a TTY stdin and stdout; use `opencode run` for piped input");
    }
    let id = super::session(process, parsed, directory).await?;
    let guard = TerminalGuard::with_options(TerminalOptions {
        mouse: false,
        kitty_keyboard: false,
    })?;
    let mut history = format!("Session {id}\n");
    if parsed.args.bool_opt("replay") != Some(false) && !parsed.args.bool("no-replay") {
        for message in process.session.context(&id).await? {
            match message {
                Message::User(user) => history.push_str(&format!("\n> {}\n", user.text)),
                Message::Assistant(assistant) => {
                    for part in assistant.content {
                        if let AssistantContent::Text(part) = part {
                            history.push_str(&part.text);
                        }
                    }
                    history.push('\n');
                }
                _ => {}
            }
        }
    }
    let mut input = String::new();
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
    if !initial.trim().is_empty() {
        submit(
            process,
            &id,
            initial,
            parsed.args.bool("thinking"),
            &guard,
            &mut history,
        )
        .await?;
    }
    draw(&history, "", &input, "Enter sends | Ctrl-C / Ctrl-D exits")?;
    let mut interval = tokio::time::interval(Duration::from_millis(25));
    loop {
        interval.tick().await;
        if guard.termination_signal().is_some() {
            break;
        }
        if !event::poll(Duration::ZERO)? {
            continue;
        }
        match UiEvent::from(event::read()?) {
            UiEvent::Key(key) if key.kind != UiKeyEventKind::Release => match key.code {
                UiKeyCode::Char('c' | 'd') if key.modifiers.control => break,
                UiKeyCode::Enter if !input.trim().is_empty() => {
                    let text = std::mem::take(&mut input);
                    submit(
                        process,
                        &id,
                        text,
                        parsed.args.bool("thinking"),
                        &guard,
                        &mut history,
                    )
                    .await?;
                }
                UiKeyCode::Backspace => {
                    input.pop();
                }
                UiKeyCode::Char(ch) if !key.modifiers.control && !key.modifiers.alt => {
                    input.push(ch)
                }
                _ => {}
            },
            UiEvent::Paste(text) => {
                input.extend(text.chars().filter(|ch| !ch.is_control() || *ch == '\n'))
            }
            UiEvent::Resize(_, _) => {}
            _ => continue,
        }
        draw(&history, "", &input, "Enter sends | Ctrl-C / Ctrl-D exits")?;
    }
    guard.restore()?;
    Ok(())
}

async fn submit(
    process: &Process,
    session: &SessionID,
    text: String,
    thinking: bool,
    guard: &TerminalGuard,
    history: &mut String,
) -> anyhow::Result<()> {
    history.push_str(&format!("\n> {text}\n\n"));
    let response = RefCell::new(String::new());
    let result = async {
        let task = turn::execute(process, session, text, thinking, |text| {
            response.borrow_mut().push_str(text);
            draw(
                history,
                &response.borrow(),
                "",
                "Running | Esc / Ctrl-C cancels",
            )
        });
        tokio::pin!(task);
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        draw(history, "", "", "Running | Esc / Ctrl-C cancels")?;
        loop {
            tokio::select! {
                result = &mut task => break result,
                _ = interval.tick() => {
                    if guard.termination_signal().is_some() { break Err(anyhow::anyhow!("Terminal terminated")); }
                    if !event::poll(Duration::ZERO)? { continue; }
                    match UiEvent::from(event::read()?) {
                        UiEvent::Key(key) if key.kind != UiKeyEventKind::Release &&
                            (key.code == UiKeyCode::Esc || key.code == UiKeyCode::Char('c') && key.modifiers.control) => {
                            break Err(anyhow::anyhow!("Session interrupted"));
                        }
                        UiEvent::Resize(_, _) => draw(history, &response.borrow(), "", "Running | Esc / Ctrl-C cancels")?,
                        _ => {}
                    }
                }
            }
        }
    }
    .await;
    if result.is_err() {
        process.execution.interrupt(session).await;
    }
    history.push_str(&response.into_inner());
    history.push('\n');
    result
}

fn draw(history: &str, response: &str, input: &str, status: &str) -> anyhow::Result<()> {
    let (width, height) = terminal::size()?;
    let clean = |text: &str| {
        text.chars()
            .filter(|ch| !ch.is_control() || *ch == '\n')
            .collect::<String>()
    };
    let body = clean(&format!("{history}{response}"));
    let lines: Vec<String> = body
        .lines()
        .flat_map(|line| {
            if line.is_empty() {
                return vec![String::new()];
            }
            line.chars()
                .collect::<Vec<_>>()
                .chunks(usize::from(width.saturating_sub(1).max(1)))
                .map(|chunk| chunk.iter().collect())
                .collect()
        })
        .collect();
    let rows = usize::from(height.saturating_sub(4));
    let mut output = std::io::stdout();
    execute!(
        output,
        cursor::MoveTo(0, 0),
        terminal::Clear(terminal::ClearType::All)
    )?;
    writeln!(output, "OpenCode | local chat\r")?;
    for line in lines.iter().skip(lines.len().saturating_sub(rows)) {
        writeln!(output, "{line}\r")?;
    }
    execute!(output, cursor::MoveTo(0, height.saturating_sub(2)))?;
    write!(output, "{}\r\n> {}", status, clean(input))?;
    output.flush()?;
    Ok(())
}
