//! Purpose: external application detection (`checkAppExists`), Windows executable resolution
//! (`resolveAppPath`, including `.cmd`/`.bat` shim parsing and sibling exe search) and opening a
//! path with the default handler or a named app (`open-path` IPC).
//! Scope: OS adapters invoking `which`/`where`/`open`/`xdg-open`/`explorer`; joined children.
//! Uses:
//! `-- none
//! External:
//! `-- tokio::{fs, process::Command}
//! Used by:
//! rust/crates/
//! `-- desktop-host/tests/platform.rs
//! Invariants: macOS checks /Applications, /System/Applications, ~/Applications, then `which`; Windows
//! and Linux report every app as present (baseline); resolution on non-Windows returns the name.
//! Contract tests: desktop-host/tests/platform.rs

use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("failed to launch {command}: {message}")]
    Spawn { command: String, message: String },
    #[error("{command} exited with code {code:?}")]
    Exit { command: String, code: Option<i32> },
}

/// Target OS for app helpers. Exposed so non-native branches are testable on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            return Self::Mac;
        }
        if cfg!(windows) {
            return Self::Windows;
        }
        Self::Linux
    }
}

/// `checkAppExists` (`packages/desktop/src/main/apps.ts:13-17`).
pub async fn check_app_exists(os: Os, app_name: &str, home: Option<&Path>) -> bool {
    match os {
        Os::Windows | Os::Linux => true,
        Os::Mac => check_macos_app(app_name, home).await,
    }
}

async fn check_macos_app(app_name: &str, home: Option<&Path>) -> bool {
    let mut locations = vec![
        PathBuf::from(format!("/Applications/{app_name}.app")),
        PathBuf::from(format!("/System/Applications/{app_name}.app")),
    ];
    if let Some(home) = home {
        locations.push(home.join(format!("Applications/{app_name}.app")));
    }
    for location in locations {
        if tokio::fs::try_exists(&location).await.unwrap_or(false) {
            return true;
        }
    }
    Command::new("which")
        .arg(app_name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

/// `resolveAppPath` (`apps.ts:19-22`): identity off Windows, `where` based resolution on Windows.
pub async fn resolve_app_path(os: Os, app_name: &str) -> Option<String> {
    if os != Os::Windows {
        return Some(app_name.to_string());
    }
    let output = Command::new("where").arg(app_name).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    resolve_windows_candidates(app_name, &paths).await
}

/// Resolution over `where` output lines (`apps.ts:39-123`), separated from the `where` call so it
/// is exercised with real shim files on every OS.
pub async fn resolve_windows_candidates(app_name: &str, paths: &[String]) -> Option<String> {
    if let Some(exe) = paths.iter().find(|path| has_ext(path, "exe")) {
        return Some(exe.clone());
    }
    for path in paths {
        if (has_ext(path, "cmd") || has_ext(path, "bat"))
            && let Some(resolved) = resolve_cmd(path).await
        {
            return Some(resolved);
        }
        if Path::new(path).extension().is_none() {
            for shim in [format!("{path}.cmd"), format!("{path}.bat")] {
                if exists(&shim).await
                    && let Some(resolved) = resolve_cmd(&shim).await
                {
                    return Some(resolved);
                }
            }
        }
    }
    let key = alnum_lower(app_name);
    if !key.is_empty() {
        for path in paths {
            let first = parent(path);
            let second = parent(&first);
            let third = parent(&second);
            for dir in [first, second, third] {
                let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                    continue;
                };
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let candidate = Path::new(&dir).join(&name).to_string_lossy().to_string();
                    if !has_ext(&candidate, "exe") {
                        continue;
                    }
                    let stem = alnum_lower(&name[..name.len() - 4]);
                    if stem.contains(&key) || key.contains(&stem) {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    paths.first().cloned()
}

async fn resolve_cmd(path: &str) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    for token in content.split('"').map(str::trim) {
        let lower = token.to_lowercase();
        if !lower.contains(".exe") {
            continue;
        }
        if let Some(index) = lower.find("%~dp0") {
            let suffix = &token[index + 5..];
            let resolved = suffix
                .replace('/', "\\")
                .split('\\')
                .filter(|part| !part.is_empty() && *part != ".")
                .fold(parent(path), |current, part| {
                    if part == ".." {
                        return parent(&current);
                    }
                    Path::new(&current).join(part).to_string_lossy().to_string()
                });
            if exists(&resolved).await {
                return Some(resolved);
            }
        }
        if exists(token).await {
            return Some(token.to_string());
        }
    }
    None
}

/// `open-path` IPC (`packages/desktop/src/main/ipc.ts:219-226`). Without an app the OS default
/// handler opens the path (Electron `shell.openPath`); with an app macOS uses `open -a`, other
/// OSes run the app with the path as its only argument.
pub async fn open_path(os: Os, path: &str, app: Option<&str>) -> Result<(), AppsError> {
    let (program, args): (&str, Vec<&str>) = match (app, os) {
        (Some(app), Os::Mac) => ("open", vec!["-a", app, path]),
        (Some(app), _) => (app, vec![path]),
        (None, Os::Mac) => ("open", vec![path]),
        (None, Os::Linux) => ("xdg-open", vec![path]),
        (None, Os::Windows) => ("explorer", vec![path]),
    };
    let status = Command::new(program)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|error| AppsError::Spawn {
            command: program.to_string(),
            message: error.to_string(),
        })?;
    // explorer.exe returns 1 even on success, so only spawn failures count on Windows defaults.
    if status.success() || (app.is_none() && os == Os::Windows) {
        return Ok(());
    }
    Err(AppsError::Exit {
        command: program.to_string(),
        code: status.code(),
    })
}

fn has_ext(path: &str, ext: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|value| value.to_string_lossy().to_lowercase() == ext)
}

fn alnum_lower(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

async fn exists(path: &str) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}
