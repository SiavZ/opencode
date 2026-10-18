//! Purpose: Slack Socket Mode bot and narrow Web API/session orchestration modules.
//! Scope: public module namespace and shared typed errors.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/session.rs
//! |-- ecosystem/src/slack/api.rs
//! |-- ecosystem/src/slack/bot.rs
//! |-- ecosystem/src/slack/output.rs
//! |-- ecosystem/src/slack/socket.rs
//! `-- ecosystem/src/slack/thread.rs
//! External:
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- ecosystem/src/lib.rs
//! Invariants: runtime tasks have owners, credentials/content are not logged.
//! Contract tests: ecosystem/tests/slack_bot.rs, ecosystem/tests/slack_socket.rs

pub mod api;
pub mod bot;
pub mod output;
pub mod socket;
pub mod thread;

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("Slack API error: {code}")]
    Api { code: String },
    #[error("Slack HTTP error: {status}")]
    Http { status: u16 },
    #[error("Slack API rate limited, retry after {retry_after} seconds")]
    RateLimited { retry_after: u64 },
    #[error("Slack transport failed: {0}")]
    Transport(String),
    #[error("Slack protocol error: {0}")]
    Protocol(&'static str),
    #[error("Slack operation timed out")]
    Timeout,
    #[error("Slack operation cancelled")]
    Cancelled,
    #[error("Slack envelope queue is full")]
    QueueFull,
    #[error("Slack thread limit reached")]
    ThreadLimit,
    #[error("Slack owned task failed")]
    Task,
    #[error(transparent)]
    Session(#[from] crate::session::SessionError),
}

impl SlackError {
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_)
                | Self::Timeout
                | Self::RateLimited { .. }
                | Self::Http { status: 500..=599 }
        )
    }
}
