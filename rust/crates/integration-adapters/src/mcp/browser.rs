//! Purpose: open an authorization URL in the user's browser, port of mcp/browser.ts plus
//! core/open.ts: only http(s) URLs, launcher failure or non-zero exit within 500ms is an error.
//! Scope: one injectable trait plus the system launcher (`open` on macOS, `xdg-open` elsewhere).
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! |-- tokio::process::Command::new
//! |-- tokio::spawn
//! |-- tokio::time::timeout
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/mcp/service.rs
//! Invariants: the launcher is never awaited past 500ms (browsers may keep the process alive);
//! the spawned launcher is reaped by a detached-but-bounded wait (kill_on_drop false).
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use std::time::Duration;

use async_trait::async_trait;

#[async_trait]
pub trait BrowserOpener: Send + Sync {
    async fn open(&self, url: &str) -> Result<(), String>;
}

pub struct SystemBrowser;

#[async_trait]
impl BrowserOpener for SystemBrowser {
    async fn open(&self, input: &str) -> Result<(), String> {
        let url = url::Url::parse(input)
            .ok()
            .filter(|u| u.scheme() == "http" || u.scheme() == "https");
        let Some(url) = url else {
            return Err(format!(
                "Only http and https links can be opened in the browser: {input}"
            ));
        };
        let launcher = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = tokio::process::Command::new(launcher)
            .arg(url.as_str())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
            Ok(Ok(status)) if !status.success() => match status.code() {
                Some(code) => Err(format!("Browser open failed with exit code {code}")),
                None => Ok(()),
            },
            Ok(Err(e)) => Err(e.to_string()),
            Ok(Ok(_)) => Ok(()),
            Err(_) => {
                // Still running after 500ms: treat as success and reap it in the background.
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                Ok(())
            }
        }
    }
}
