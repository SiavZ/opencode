//! Purpose: navigation origin and external URL policy, renderer crash and unresponsive recovery policy.
//! Scope: pure time-injected policy state machine for window navigation and recovery.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/windows.rs
//! `-- desktop-host/tests/windows_policy.rs
//! Invariants: pure functions, no side effects, time injection for testing.
//! Contract tests: desktop-host/tests/windows_policy.rs

use serde::{Deserialize, Serialize};
use url::Url;

/// Supported protocols for external URL resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalProtocol {
    Http,
    Https,
    Mailto,
}

/// Result of resolving a URL for external opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalUrlResolution {
    Allowed {
        url: String,
        protocol: ExternalProtocol,
    },
    Denied,
}

/// Result of resolving a URL for local file path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalFilePathResolution {
    Allowed { path: String },
    Denied,
}

/// Resolves an external URL exactly matching TS `resolveExternalURL`.
pub fn resolve_external_url(value: &str) -> ExternalUrlResolution {
    if let Ok(url) = Url::parse(value) {
        let protocol = url.scheme();
        if protocol == "http" || protocol == "https" || protocol == "mailto" {
            return ExternalUrlResolution::Allowed {
                url: url.to_string(),
                protocol: match protocol {
                    "http" => ExternalProtocol::Http,
                    "https" => ExternalProtocol::Https,
                    "mailto" => ExternalProtocol::Mailto,
                    _ => unreachable!(),
                },
            };
        }
    }
    ExternalUrlResolution::Denied
}

/// Resolves a local file path exactly matching TS `resolveLocalFilePath`.
pub fn resolve_local_file_path(value: &str) -> LocalFilePathResolution {
    if let Ok(url) = Url::parse(value)
        && url.scheme() == "file"
        && url.host().is_none()
        && let Ok(path) = url.to_file_path()
    {
        return LocalFilePathResolution::Allowed {
            path: path.to_string_lossy().to_string(),
        };
    }
    LocalFilePathResolution::Denied
}

/// Renderer protocol configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RendererConfig {
    pub protocol: String,
    pub host: String,
    pub dev_url_origin: Option<String>,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            protocol: "oc".to_string(),
            host: "renderer".to_string(),
            dev_url_origin: None,
        }
    }
}

/// Checks if a URL is a trusted renderer URL.
pub fn is_trusted_renderer_url(value: &str, config: &RendererConfig, require_html: bool) -> bool {
    if let Ok(url) = Url::parse(value) {
        if require_html && !url.path().ends_with(".html") {
            return false;
        }
        if url.scheme() == config.protocol && url.host_str() == Some(&config.host) {
            return true;
        }
        if let Some(dev_origin) = &config.dev_url_origin
            && let Ok(dev_url) = Url::parse(dev_origin)
            && url.origin() == dev_url.origin()
        {
            return true;
        }
    }
    false
}

/// Navigation policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationDecision {
    AllowInWindow,
    DenyAndOpenExternal { url: String },
}

/// Evaluates navigation policy for a URL.
pub fn evaluate_navigation(url: &str, config: &RendererConfig) -> NavigationDecision {
    if is_trusted_renderer_url(url, config, false) {
        NavigationDecision::AllowInWindow
    } else {
        NavigationDecision::DenyAndOpenExternal {
            url: url.to_string(),
        }
    }
}

/// Permission types allowed in the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererPermission {
    ClipboardSanitizedWrite,
    Notifications,
}

impl RendererPermission {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "clipboard-sanitized-write" => Some(Self::ClipboardSanitizedWrite),
            "notifications" => Some(Self::Notifications),
            _ => None,
        }
    }
}

/// Permission check decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

/// Evaluates permission request.
pub fn evaluate_permission(
    permission: &str,
    requesting_url: &str,
    web_contents_id: u32,
    target_web_contents_id: u32,
    config: &RendererConfig,
) -> PermissionDecision {
    if RendererPermission::parse_str(permission).is_none() {
        return PermissionDecision::Deny;
    }

    if web_contents_id != target_web_contents_id {
        return PermissionDecision::Deny;
    }

    if is_trusted_renderer_url(requesting_url, config, false) {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Deny
    }
}

/// Recovery action available to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAction {
    Relaunch,
    ExportLogs,
    KeepWaiting,
    Quit,
}

/// Event triggering recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryEvent {
    RenderProcessGone {
        reason: String,
        exit_code: Option<i32>,
    },
    Unresponsive,
    Responsive,
    LoadFailed {
        error_code: i32,
        error_description: String,
        validated_url: String,
        is_main_frame: bool,
    },
}

/// State of the recovery state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Idle,
    Sampling { sample_count: u32 },
    ShowingDialog { wait_allowed: bool },
}

/// Result of processing a recovery event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryResult {
    NoAction,
    StartSampling,
    StopSamplingAndFlush {
        samples: Vec<(String, u32)>,
    },
    ShowDialog {
        message: String,
        detail: String,
        wait_allowed: bool,
    },
    ExecuteAction(RecoveryAction),
}

/// Unresponsive sampler configuration with time injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresponsiveSamplerConfig {
    pub sample_interval_ms: u64,
    pub sample_period_ms: u64,
}

impl Default for UnresponsiveSamplerConfig {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1000,
            sample_period_ms: 15000,
        }
    }
}

/// Pure state machine for unresponsive sampling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresponsiveSampler {
    config: UnresponsiveSamplerConfig,
    state: RecoveryState,
    samples: std::collections::HashMap<String, u32>,
    is_dev_tools_open: bool,
}

impl UnresponsiveSampler {
    pub fn new(config: UnresponsiveSamplerConfig) -> Self {
        Self {
            config,
            state: RecoveryState::Idle,
            samples: std::collections::HashMap::new(),
            is_dev_tools_open: false,
        }
    }

    pub fn set_dev_tools_open(&mut self, open: bool) {
        self.is_dev_tools_open = open;
    }

    pub fn start(&mut self) -> bool {
        if matches!(
            self.state,
            RecoveryState::Sampling { .. } | RecoveryState::ShowingDialog { .. }
        ) || self.is_dev_tools_open
        {
            return false;
        }
        self.state = RecoveryState::Sampling { sample_count: 0 };
        self.samples.clear();
        true
    }

    pub fn collect_sample(&mut self, stack: Option<String>) -> bool {
        if !matches!(self.state, RecoveryState::Sampling { .. }) {
            return false;
        }
        if let Some(stack) = stack {
            *self.samples.entry(stack).or_insert(0) += 1;
        }
        if let RecoveryState::Sampling { sample_count } = &mut self.state {
            *sample_count += 1;
        }
        true
    }

    pub fn stop_and_flush(&mut self) -> (bool, Vec<(String, u32)>) {
        let was_sampling = matches!(self.state, RecoveryState::Sampling { .. });
        self.state = RecoveryState::Idle;

        if self.samples.is_empty() {
            return (was_sampling, Vec::new());
        }

        let mut entries: Vec<_> = self.samples.drain().collect();
        entries.sort_by_key(|a| std::cmp::Reverse(a.1));
        (was_sampling, entries)
    }

    pub fn check_timeout(&self, elapsed_ms: u64) -> bool {
        matches!(self.state, RecoveryState::Sampling { .. })
            && elapsed_ms >= self.config.sample_period_ms
    }
}

/// Evaluates recovery event and returns the required action.
pub fn evaluate_recovery_event(
    event: &RecoveryEvent,
    current_state: &mut RecoveryState,
) -> RecoveryResult {
    match event {
        RecoveryEvent::RenderProcessGone { reason, exit_code } => {
            *current_state = RecoveryState::ShowingDialog {
                wait_allowed: false,
            };
            RecoveryResult::ShowDialog {
                message: "Renderer process terminated".to_string(),
                detail: format!("Reason: {}, Code: {}", reason, exit_code.unwrap_or(-1)),
                wait_allowed: false,
            }
        }
        RecoveryEvent::Unresponsive => {
            *current_state = RecoveryState::ShowingDialog { wait_allowed: true };
            RecoveryResult::ShowDialog {
                message: "Renderer unresponsive".to_string(),
                detail: "The renderer is not responding.".to_string(),
                wait_allowed: true,
            }
        }
        RecoveryEvent::Responsive => {
            *current_state = RecoveryState::Idle;
            RecoveryResult::StopSamplingAndFlush {
                samples: Vec::new(),
            }
        }
        RecoveryEvent::LoadFailed {
            error_code,
            error_description,
            validated_url,
            is_main_frame,
        } => {
            if !is_main_frame || *error_code == -3 {
                return RecoveryResult::NoAction;
            }
            *current_state = RecoveryState::ShowingDialog {
                wait_allowed: false,
            };
            RecoveryResult::ShowDialog {
                message: "Renderer load failed".to_string(),
                detail: format!(
                    "URL: {}, Code: {}, Description: {}",
                    validated_url, error_code, error_description
                ),
                wait_allowed: false,
            }
        }
    }
}

/// Evaluates user action during recovery dialog.
pub fn evaluate_recovery_action(
    action: RecoveryAction,
    wait_allowed: bool,
    was_sampling: bool,
) -> RecoveryResult {
    match action {
        RecoveryAction::ExportLogs => {
            if wait_allowed && was_sampling {
                RecoveryResult::StartSampling
            } else {
                RecoveryResult::NoAction
            }
        }
        RecoveryAction::Relaunch => RecoveryResult::ExecuteAction(RecoveryAction::Relaunch),
        RecoveryAction::Quit => RecoveryResult::ExecuteAction(RecoveryAction::Quit),
        RecoveryAction::KeepWaiting => RecoveryResult::NoAction,
    }
}
