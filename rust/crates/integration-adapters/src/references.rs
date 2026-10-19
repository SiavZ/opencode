//! Purpose: parse remote repository references and maintain branch-isolated tracking checkouts.
//! Scope: native Git cache beneath an explicit integration cache directory.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- fs4::fs_std::FileExt
//! |-- tokio::process::Command
//! `-- url::Url
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations/resources.rs
//! Invariants: invalid branches never reach Git; refresh hard-resets only validated cache checkouts.
//! Contract tests: integration-adapters/tests/reference_cache.rs

use opencode_integration_api::IntegrationError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub host: String,
    pub segments: Vec<String>,
    pub remote: String,
}
impl Repository {
    pub fn parse(input: &str, github_base: Option<&str>) -> Option<Self> {
        let input = input
            .trim()
            .strip_prefix("git+")
            .unwrap_or(input.trim())
            .split('#')
            .next()?
            .trim_end_matches('/');
        let (host, parts, original) = if let Some(path) = input.strip_prefix("github:") {
            ("github.com".to_owned(), path.to_owned(), None)
        } else if input.contains("://") {
            let url = Url::parse(input).ok()?;
            if url.scheme() == "file" {
                return None;
            }
            (
                format!(
                    "{}{}",
                    url.host_str()?,
                    url.port()
                        .map(|port| format!(":{port}"))
                        .unwrap_or_default()
                ),
                url.path().into(),
                Some(input.to_owned()),
            )
        } else if let Some((host, path)) = input.split_once(':') {
            (
                host.rsplit('@').next()?.into(),
                path.into(),
                Some(input.to_owned()),
            )
        } else {
            let parts: Vec<_> = input.split('/').collect();
            if parts.len() == 2 && !parts[0].contains('.') {
                ("github.com".into(), input.into(), None)
            } else {
                (parts.first()?.to_string(), parts.get(1..)?.join("/"), None)
            }
        };
        let host = host.to_lowercase();
        let segments: Vec<String> = parts
            .split('/')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| part.strip_suffix(".git").unwrap_or(part).to_owned())
            .collect();
        if host.is_empty()
            || matches!(host.as_str(), "." | "..")
            || host.starts_with('-')
            || host
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '\0'))
            || segments.is_empty()
            || segments.iter().any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || part.contains(':')
                    || part
                        .chars()
                        .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '\0'))
            })
        {
            return None;
        }
        let path = segments.join("/");
        let remote = if host == "github.com" {
            github_base
                .map(|base| format!("{}/{path}.git", base.trim_end_matches('/')))
                .unwrap_or_else(|| format!("https://github.com/{path}.git"))
        } else {
            original.unwrap_or_else(|| format!("https://{host}/{path}.git"))
        };
        Some(Self {
            host,
            segments,
            remote,
        })
    }
    pub fn cache_path(
        &self,
        cache: &Path,
        branch: Option<&str>,
    ) -> Result<PathBuf, IntegrationError> {
        if let Some(branch) = branch {
            validate_branch(branch)?;
        }
        let path = self
            .host
            .split(':')
            .chain(self.segments.iter().map(String::as_str))
            .fold(cache.to_owned(), |path, part| path.join(part));
        Ok(match branch {
            Some(branch) => PathBuf::from(format!("{}@{}", path.display(), encode(branch))),
            None => path,
        })
    }
}

pub fn validate_branch(branch: &str) -> Result<(), IntegrationError> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.contains("..")
        || !branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
    {
        return Err(IntegrationError::Validation("Branch must contain only alphanumeric characters, /, _, ., and -, and cannot start with - or contain ..".into()));
    }
    Ok(())
}

pub async fn ensure(
    repository: &Repository,
    cache: &Path,
    branch: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf, IntegrationError> {
    let path = repository.cache_path(cache, branch)?;
    let parent = path
        .parent()
        .ok_or_else(|| IntegrationError::Validation("repository cache parent".into()))?;
    tokio::fs::create_dir_all(cache).await.map_err(io_error)?;
    let root = tokio::fs::canonicalize(cache).await.map_err(io_error)?;
    tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    if !tokio::fs::canonicalize(parent)
        .await
        .map_err(io_error)?
        .starts_with(&root)
        || tokio::fs::symlink_metadata(&path)
            .await
            .is_ok_and(|entry| entry.file_type().is_symlink())
    {
        return Err(IntegrationError::Validation(
            "reference cache crosses a symbolic link boundary".into(),
        ));
    }
    let lockpath = path.with_extension("opencode-lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lockpath)
        .map_err(io_error)?;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(()),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(io_error(error)),
            }
        }
    })
    .await
    .map_err(|_| IntegrationError::Timeout { ms: 60_000 })??;
    let result = update(repository, &path, branch, environment).await;
    drop(lock);
    result.map(|()| path)
}

async fn update(
    repository: &Repository,
    path: &Path,
    branch: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<(), IntegrationError> {
    let reusable = if path.exists() {
        let worktree = git(path, &["rev-parse", "--show-toplevel"], environment)
            .await
            .ok();
        let origin = git(path, &["remote", "get-url", "origin"], environment)
            .await
            .ok();
        worktree.is_some_and(|worktree| {
            std::fs::canonicalize(worktree.trim()).ok() == std::fs::canonicalize(path).ok()
        }) && origin
            .and_then(|origin| Repository::parse(origin.trim(), None))
            .is_some_and(|origin| {
                origin.host == repository.host && origin.segments == repository.segments
            })
    } else {
        false
    };
    if !reusable {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.map_err(io_error)?;
        }
        let mut args = vec!["clone".to_owned()];
        if let Some(branch) = branch {
            args.extend(["--branch".into(), branch.into()]);
        }
        args.extend([
            "--".into(),
            repository.remote.clone(),
            path.display().to_string(),
        ]);
        git(
            path.parent().unwrap_or(cache_root()),
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            environment,
        )
        .await?;
        return Ok(());
    }
    git(path, &["fetch", "--all", "--prune"], environment).await?;
    let branch = match branch {
        Some(branch) => Some(branch.into()),
        None => git(
            path,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            environment,
        )
        .await
        .ok()
        .map(|branch| {
            branch
                .trim()
                .strip_prefix("origin/")
                .unwrap_or(branch.trim())
                .to_owned()
        }),
    };
    if let Some(branch) = branch {
        git(
            path,
            &["checkout", "-B", &branch, &format!("origin/{branch}")],
            environment,
        )
        .await?;
        git(
            path,
            &["reset", "--hard", &format!("origin/{branch}")],
            environment,
        )
        .await?;
        return Ok(());
    }
    git(path, &["reset", "--hard", "HEAD"], environment).await?;
    Ok(())
}
fn cache_root() -> &'static Path {
    Path::new("/")
}
async fn git(
    directory: &Path,
    args: &[&str],
    environment: &BTreeMap<String, String>,
) -> Result<String, IntegrationError> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new("git")
            .args(args)
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| IntegrationError::Timeout { ms: 60_000 })?
    .map_err(io_error)?;
    if !output.status.success() {
        return Err(IntegrationError::Failed {
            name: "reference Git".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().into(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
fn encode(input: &str) -> String {
    input
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn io_error(error: std::io::Error) -> IntegrationError {
    IntegrationError::Failed {
        name: "reference cache".into(),
        message: error.to_string(),
    }
}
