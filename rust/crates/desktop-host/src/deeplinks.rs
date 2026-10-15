//! Purpose: deep link validation, pending delivery and window routing.
//! Scope: scheme validation, argv extraction, pending queue, consume_initial, routing, sink trait.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/deeplinks.rs
//! External:
//! |-- url::Url
//! `-- serde::{Serialize, Deserialize}
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/lib.rs
//! Invariants: baseline ordering and duplicate behavior preserved exactly (pending is always appended, delivery is best-effort to focused window).
//! Contract tests: desktop-host/tests/deeplinks.rs

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;

/// Parsed deep link target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeepLinkTarget {
    OpenProject {
        directory: String,
    },
    NewSession {
        directory: String,
        prompt: Option<String>,
    },
}

/// Error returned when parsing a deep link fails.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DeepLinkError {
    #[error("invalid scheme, must start with opencode://")]
    InvalidScheme,
    #[error("invalid URL")]
    InvalidUrl,
    #[error("missing or empty directory parameter")]
    MissingDirectory,
    #[error("unsupported hostname: {0}")]
    UnsupportedHostname(String),
}

/// Parse a single deep link string into a structured target.
/// Preserves baseline validation: starts with "opencode://", valid URL,
/// hostname is "open-project" or "new-session", and "directory" is present and non-empty.
pub fn parse_deep_link(input: &str) -> Result<DeepLinkTarget, DeepLinkError> {
    if !input.starts_with("opencode://") {
        return Err(DeepLinkError::InvalidScheme);
    }

    let url = Url::parse(input).map_err(|_| DeepLinkError::InvalidUrl)?;

    match url.host_str() {
        Some("open-project") => {
            let directory = url
                .query_pairs()
                .find(|(k, _)| k == "directory")
                .map(|(_, v)| v.into_owned())
                .ok_or(DeepLinkError::MissingDirectory)?;

            if directory.is_empty() {
                return Err(DeepLinkError::MissingDirectory);
            }

            Ok(DeepLinkTarget::OpenProject { directory })
        }
        Some("new-session") => {
            let directory = url
                .query_pairs()
                .find(|(k, _)| k == "directory")
                .map(|(_, v)| v.into_owned())
                .ok_or(DeepLinkError::MissingDirectory)?;

            if directory.is_empty() {
                return Err(DeepLinkError::MissingDirectory);
            }

            let prompt = url
                .query_pairs()
                .find(|(k, _)| k == "prompt")
                .map(|(_, v)| v.into_owned())
                .filter(|v| !v.is_empty());

            Ok(DeepLinkTarget::NewSession { directory, prompt })
        }
        Some(other) => Err(DeepLinkError::UnsupportedHostname(other.to_string())),
        None => Err(DeepLinkError::InvalidUrl),
    }
}

/// Trait for delivering deep links to a window or process.
pub trait DeepLinkSink {
    /// Send deep links to the specified window.
    fn send_deep_links(&self, window_id: &str, urls: &[String]);
    /// Get the ID of the currently focused window, if any.
    fn get_focused_window(&self) -> Option<String>;
    /// Get the ID of the most recently used window, if any.
    fn get_most_recent_window(&self) -> Option<String>;
}

/// Manages pending deep links and routing.
/// Preserves baseline behavior: pending queue is ALWAYS appended to,
/// and delivery to a window is attempted if one exists, without removing from pending.
pub struct DeepLinkManager<S: DeepLinkSink> {
    pending: Arc<Mutex<Vec<String>>>,
    sink: S,
}

impl<S: DeepLinkSink> DeepLinkManager<S> {
    pub fn new(sink: S) -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            sink,
        }
    }

    /// Add deep links to the pending queue and attempt to route them immediately.
    /// Preserves baseline duplicate behavior: links are always appended to pending,
    /// and also sent to the focused (or most recent) window if one exists.
    pub fn emit_deep_links(&self, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }

        let mut pending = self.pending.lock();
        pending.extend(urls.clone());

        if let Some(window_id) = self
            .sink
            .get_focused_window()
            .or_else(|| self.sink.get_most_recent_window())
        {
            self.sink.send_deep_links(&window_id, &urls);
        }
    }

    /// Drain and return all pending deep links, clearing the queue (splice semantics).
    pub fn consume_initial(&self) -> Vec<String> {
        let mut pending = self.pending.lock();
        let result = pending.clone();
        pending.clear();
        result
    }

    /// Extract deep link URLs from command-line arguments (Windows/Linux second-instance behavior).
    pub fn extract_from_argv(argv: &[String]) -> Vec<String> {
        argv.iter()
            .filter(|arg| arg.starts_with("opencode://"))
            .cloned()
            .collect()
    }
}
