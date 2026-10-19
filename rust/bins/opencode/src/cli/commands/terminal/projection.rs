//! Purpose: adapt durable session messages into the terminal timeline's typed widgets.
//! Scope: pure projection of one history snapshot.
//! Uses:
//! rust/crates/
//! |-- session-api/src/message.rs
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! `-- serde_json::to_value
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: projection rejects unknown states; tool failures and reasoning retain their types.
//! Contract tests: opencode/tests/commands.rs terminal_app module

use opencode_ui_terminal::timeline::{Message, Part, Timeline};

pub fn project(
    messages: &[opencode_session_api::message::Message],
    thinking: bool,
    timeline: &mut Timeline,
) -> anyhow::Result<()> {
    timeline.set_messages(
        messages
            .iter()
            .map(|message| -> anyhow::Result<Message> {
                let mut message = Message::from_value(&serde_json::to_value(message)?)?;
                if !thinking {
                    message
                        .parts
                        .retain(|part| !matches!(part, Part::Reasoning(_)));
                }
                Ok(message)
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    Ok(())
}
