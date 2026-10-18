//! Purpose: typed configuration errors mirroring the baseline `NamedError`s
//! (`ConfigJsonError`, `ConfigInvalidError`, `ConfigFrontmatterError`, `ConfigRemoteAuthError`)
//! plus typed transport/IO/unsupported paths.
//! Scope: error values.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::{Value, json}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- config/src/lib.rs
//! `-- config/src/variable.rs
//! Invariants: `to_named_json` matches the `{ name, data }` wire shape of `NamedError.toObject`;
//! issue lists never embed config values (secrets stay out of errors).
//! Contract tests: config/tests/legacy_update.rs

use serde_json::{Value, json};

/// One schema issue: dotted path segments plus a message, like the baseline formatter output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub path: Vec<String>,
    pub message: String,
}

impl Issue {
    pub fn new(path: &[String], message: impl Into<String>) -> Self {
        Self {
            path: path.to_vec(),
            message: message.into(),
        }
    }
}

/// Why a remote/org config fetch failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// No transport is wired (Phase 03/06 own HTTP). Default for `UnsupportedRemote`.
    #[error("remote config transport is not available: {0}")]
    Unsupported(String),
    #[error("failed to fetch remote config from {url}: {message}")]
    Fetch { url: String, message: String },
    #[error("failed to decode remote config from {url}: {message}")]
    Decode { url: String, message: String },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// `ConfigJsonError`: the file was not valid JSONC.
    #[error("Config file at {path} is not valid JSON(C): {message}")]
    Json { path: String, message: String },
    /// `ConfigInvalidError`: schema validation, bad `{file:}` references, native permissions.
    #[error("Configuration is invalid at {path}{}", message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Invalid {
        path: String,
        issues: Vec<Issue>,
        message: Option<String>,
    },
    /// `ConfigFrontmatterError`.
    #[error("{message}")]
    Frontmatter { path: String, message: String },
    /// `ConfigRemoteAuthError`: an auth proxy answered with an HTML login page.
    #[error("Remote config at {remote} requires re-authentication with {url}")]
    RemoteAuth { url: String, remote: String },
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// IO failures other than not-found (which the loader treats as absent).
    #[error("{operation} {path}: {message}")]
    Io {
        operation: &'static str,
        path: String,
        message: String,
        permission_denied: bool,
    },
    #[error("JSONC edit failed for {path}: {message}")]
    Edit { path: String, message: String },
}

impl ConfigError {
    pub fn io(operation: &'static str, path: &std::path::Path, error: &std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.display().to_string(),
            message: error.to_string(),
            permission_denied: error.kind() == std::io::ErrorKind::PermissionDenied,
        }
    }

    pub fn invalid_message(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            issues: Vec::new(),
            message: Some(message.into()),
        }
    }

    /// The baseline `NamedError` name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Json { .. } => "ConfigJsonError",
            Self::Invalid { .. } => "ConfigInvalidError",
            Self::Frontmatter { .. } => "ConfigFrontmatterError",
            Self::RemoteAuth { .. } => "ConfigRemoteAuthError",
            Self::Remote(_) => "ConfigRemoteError",
            Self::Io { .. } => "ConfigIOError",
            Self::Edit { .. } => "ConfigEditError",
        }
    }

    /// `{ name, data }` like `NamedError.toObject()`.
    pub fn to_named_json(&self) -> Value {
        let data = match self {
            Self::Json { path, message } => json!({ "path": path, "message": message }),
            Self::Invalid {
                path,
                issues,
                message,
            } => {
                let mut data = json!({ "path": path });
                if !issues.is_empty() {
                    data["issues"] = Value::Array(
                        issues
                            .iter()
                            .map(|issue| json!({ "message": issue.message, "path": issue.path }))
                            .collect(),
                    );
                }
                if let Some(message) = message {
                    data["message"] = json!(message);
                }
                data
            }
            Self::Frontmatter { path, message } => json!({ "path": path, "message": message }),
            Self::RemoteAuth { url, remote } => json!({ "url": url, "remote": remote }),
            Self::Remote(error) => json!({ "message": error.to_string() }),
            Self::Io { path, message, .. } => json!({ "path": path, "message": message }),
            Self::Edit { path, message } => json!({ "path": path, "message": message }),
        };
        json!({ "name": self.name(), "data": data })
    }
}
