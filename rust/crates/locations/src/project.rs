//! Purpose: project identity resolution (port of packages/core/src/project.ts:65-128 `ProjectV2.resolve`
//! and `commit`): git remote hash, cached id in `<common dir>/opencode`, root commit, `global` fallback.
//! Scope: `ProjectResolver` trait, `Resolved`, `GitProjectResolver`, remote URL normalization.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/git.rs
//! `-- schema/src/ids.rs
//! External:
//! |-- async_trait::async_trait
//! |-- hex::encode
//! |-- regex::Regex
//! |-- regex::Regex::new
//! |-- sha1::Digest
//! |-- sha1::Sha1
//! |-- tokio::fs::read_to_string
//! |-- tokio::fs::write
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/project_store.rs
//! `-- locations/tests/project.rs
//! Invariants: id precedence is remote hash, then cached id, then first sorted root commit, then
//! `global`; non-git input resolves to `{ id: global, directory: filesystem root, vcs: None }`;
//! `file:` remotes and unparseable remotes never produce an id; `commit` ignores write failures.
//! Contract tests: locations/tests/project.rs

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use opencode_schema::ids::ProjectID;
use sha1::{Digest, Sha1};

use crate::error::LocationError;
use crate::git::GitRunner;

/// `ProjectSchema.Vcs` (`{ type: "git", store }`). `store` is the git common directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vcs {
    pub store: PathBuf,
}

impl Vcs {
    pub const TYPE: &'static str = "git";
}

/// `Project.Resolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Id read from the repo-local cache file, before migration.
    pub previous: Option<ProjectID>,
    pub id: ProjectID,
    pub directory: PathBuf,
    pub vcs: Option<Vcs>,
}

#[async_trait]
pub trait ProjectResolver: Send + Sync {
    /// `ProjectV2.resolve(directory)`.
    async fn resolve(&self, directory: &str) -> Result<Resolved, LocationError>;
    /// `ProjectV2.commit({ store, id })`: write the id cache. Failures are ignored.
    async fn commit(&self, store: &Path, id: &ProjectID);
}

/// Resolver backed by a `GitRunner`.
#[derive(Debug, Clone)]
pub struct GitProjectResolver {
    git: Arc<dyn GitRunner>,
}

impl GitProjectResolver {
    pub fn new(git: Arc<dyn GitRunner>) -> Self {
        Self { git }
    }
}

#[async_trait]
impl ProjectResolver for GitProjectResolver {
    async fn resolve(&self, directory: &str) -> Result<Resolved, LocationError> {
        let input = Path::new(directory);
        let Some(repo) = self.git.discover(input).await else {
            return Ok(Resolved {
                previous: None,
                id: ProjectID::global(),
                directory: filesystem_root(input),
                vcs: None,
            });
        };
        let previous = cached(&repo.common_directory).await;
        let remote = match self.git.remote_url(&repo, "origin").await {
            Some(origin) => normalize_remote(&origin).map(|n| ProjectID::new(remote_id(&n))),
            None => None,
        };
        let id = match remote.or_else(|| previous.clone()) {
            Some(id) => Some(id),
            None => self
                .git
                .root_commits(&repo)
                .await
                .into_iter()
                .next()
                .map(ProjectID::new),
        };
        Ok(Resolved {
            previous,
            id: id.unwrap_or_else(ProjectID::global),
            directory: repo.worktree,
            vcs: Some(Vcs {
                store: repo.common_directory,
            }),
        })
    }

    async fn commit(&self, store: &Path, id: &ProjectID) {
        let _ = tokio::fs::write(store.join("opencode"), id.as_str()).await;
    }
}

async fn cached(dir: &Path) -> Option<ProjectID> {
    let value = tokio::fs::read_to_string(dir.join("opencode")).await.ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| ProjectID::new(value))
}

/// `path.parse(input).root`.
fn filesystem_root(input: &Path) -> PathBuf {
    input
        .ancestors()
        .last()
        .filter(|root| root.has_root())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// `Hash.fast("git-remote:" + normalized)` (sha1 hex).
pub fn remote_id(normalized: &str) -> String {
    hex::encode(Sha1::digest(format!("git-remote:{normalized}").as_bytes()))
}

static SCP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^([^@/:]+@)?([^/:]+):(.+)$").expect("valid regex"));

/// `url(input)` of project.ts: `host/path` with lowercase host and no leading slash, `.git` or
/// trailing slash. `None` for `file:` URLs and inputs that are neither URLs nor scp-like.
pub fn normalize_remote(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    match url::Url::parse(value) {
        Ok(parsed) => {
            if parsed.scheme() == "file" {
                return None;
            }
            parts(parsed.host_str().unwrap_or(""), parsed.path())
        }
        Err(_) => {
            let captures = SCP.captures(value)?;
            parts(&captures[2], &captures[3])
        }
    }
}

fn parts(host: &str, name: &str) -> Option<String> {
    let pathname = name.trim_start_matches('/');
    let pathname = pathname
        .strip_suffix(".git/")
        .or_else(|| pathname.strip_suffix(".git"))
        .unwrap_or(pathname);
    let pathname = pathname.trim_end_matches('/');
    if host.is_empty() || pathname.is_empty() {
        return None;
    }
    Some(format!("{}/{pathname}", host.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remotes_like_baseline() {
        let cases = [
            (
                "https://GitHub.com/acme/repo.git",
                Some("github.com/acme/repo"),
            ),
            ("git@github.com:acme/repo.git", Some("github.com/acme/repo")),
            (
                "ssh://git@github.com/acme/repo",
                Some("github.com/acme/repo"),
            ),
            ("github.com:acme/repo/", None),
            ("file:///tmp/repo", None),
            ("   ", None),
            ("/local/path", None),
            ("https://host/acme/repo.git/", Some("host/acme/repo")),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_remote(input).as_deref(), expected, "{input}");
        }
        assert_eq!(
            remote_id("github.com/acme/repo"),
            hex::encode(Sha1::digest(b"git-remote:github.com/acme/repo"))
        );
    }
}
