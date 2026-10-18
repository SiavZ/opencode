//! Purpose: index and tree operations of packages/core/src/git.ts (`index.refresh`, `index.ignored`,
//! `tree.capture/write/files/diff`) over a repository with an explicit git dir and work tree.
//! Restore-side operations live in `tree_restore.rs` and are re-exported here.
//! Scope: free functions over `GitCli`. Locking is per git dir for mutating tree operations.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/run.rs
//! `-- platform-native/src/vcs/tree_restore.rs
//! External:
//! `-- tokio::try_join
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/repo.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- platform-native/src/vcs/tree_restore.rs
//! `-- platform-native/tests/git.rs
//! Invariants: refresh touches only the requested scope; ignored candidates and untracked files
//! above the size limit are removed from the index, never staged; capture holds the git-dir lock.
//! Contract tests: platform-native/tests/snapshot.rs, platform-native/tests/git.rs

use super::error::{Operation, VcsError};
use super::run::{GitCli, GitOutput, nul_join, nul_split};
use opencode_platform_api::git::Repository;
use opencode_platform_api::snapshot::{DiffStatus, FileDiff};
use std::collections::{BTreeSet, HashSet};

pub use super::tree_restore::{checkout, preview, remove_path, restore};

/// `repositoryOperation`: run with `--git-dir/--work-tree`, fail with an `OperationError`.
pub async fn repo_op<S: AsRef<str>>(
    cli: &GitCli,
    operation: Operation,
    repository: &Repository,
    args: &[S],
    stdin: Option<Vec<u8>>,
) -> Result<GitOutput, VcsError> {
    repo_op_env(cli, operation, repository, args, stdin, &[]).await
}

pub async fn repo_op_env<S: AsRef<str>>(
    cli: &GitCli,
    operation: Operation,
    repository: &Repository,
    args: &[S],
    stdin: Option<Vec<u8>>,
    env: &[(&str, String)],
) -> Result<GitOutput, VcsError> {
    let result = cli
        .repo(repository, args, stdin, env)
        .await
        .map_err(|error| VcsError::operation(operation, error.to_string()))?;
    if result.ok() {
        return Ok(result);
    }
    let fallback = format!("Git {} failed", operation.as_str());
    Err(VcsError::operation(operation, result.message(&fallback)))
}

/// `Git.index.refresh`: stage changed tracked and untracked files under `scope`, dropping ignored
/// and oversized untracked files from the index. Returns the skipped oversized paths.
pub async fn refresh(
    cli: &GitCli,
    repository: &Repository,
    scope: &str,
    ignores: Option<&Repository>,
    maximum_untracked_bytes: Option<u64>,
) -> Result<Vec<String>, VcsError> {
    let list = |args: Vec<&'static str>| async move {
        let mut full: Vec<String> = args.into_iter().map(str::to_string).collect();
        full.extend(["--".to_string(), scope.to_string()]);
        repo_op(cli, Operation::Refresh, repository, &full, None)
            .await
            .map(|out| nul_split(&out.text))
    };
    let (tracked, untracked) = tokio::try_join!(
        list(vec!["diff-files", "--name-only", "-z"]),
        list(vec!["ls-files", "--others", "--exclude-standard", "-z"]),
    )?;
    let mut seen = HashSet::new();
    let candidates: Vec<String> = tracked
        .iter()
        .chain(untracked.iter())
        .filter(|item| seen.insert(item.to_string()))
        .cloned()
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let ignored: Vec<String> = match ignores {
        Some(source) => {
            let args = ["check-ignore", "--no-index", "--stdin", "-z"];
            let stdin = Some(nul_join(&candidates));
            repo_op(cli, Operation::Refresh, source, &args, stdin)
                .await
                .map(|out| nul_split(&out.text))
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    let ignored_set: HashSet<&String> = ignored.iter().collect();
    let allowed: Vec<&String> = candidates
        .iter()
        .filter(|item| !ignored_set.contains(item))
        .collect();
    let mut skipped = Vec::new();
    if let Some(maximum) = maximum_untracked_bytes.filter(|max| *max > 0) {
        for item in untracked.iter().filter(|item| allowed.contains(item)) {
            let Ok(info) = tokio::fs::metadata(repository.root.join(item)).await else {
                continue;
            };
            if info.is_file() && info.len() > maximum {
                skipped.push(item.clone());
            }
        }
    }
    let stage: Vec<&String> = allowed
        .into_iter()
        .filter(|item| !skipped.contains(item))
        .collect();
    let remove: Vec<&String> = ignored.iter().chain(skipped.iter()).collect();
    if !remove.is_empty() {
        let args = [
            "rm",
            "--cached",
            "-f",
            "--ignore-unmatch",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ];
        repo_op(
            cli,
            Operation::Refresh,
            repository,
            &args,
            Some(nul_join(&remove)),
        )
        .await?;
    }
    if !stage.is_empty() {
        let args = [
            "add",
            "--all",
            "--sparse",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ];
        repo_op(
            cli,
            Operation::Refresh,
            repository,
            &args,
            Some(nul_join(&stage)),
        )
        .await?;
    }
    Ok(skipped)
}

/// `Git.index.ignored`: paths ignored by `repository`'s ignore rules (exit 1 means none).
pub async fn ignored(
    cli: &GitCli,
    repository: &Repository,
    paths: &[String],
) -> Result<BTreeSet<String>, VcsError> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let args = ["check-ignore", "--no-index", "--stdin", "-z"];
    let result = cli
        .repo(repository, &args, Some(nul_join(paths)), &[])
        .await
        .map_err(|error| VcsError::operation(Operation::ListFiles, error.to_string()))?;
    if result.code != 0 && result.code != 1 {
        let message = match result.stderr.trim() {
            "" => "Failed to check ignored paths".to_string(),
            stderr => stderr.to_string(),
        };
        return Err(VcsError::operation(Operation::ListFiles, message));
    }
    Ok(nul_split(&result.text).into_iter().collect())
}

/// `Git.tree.write`.
pub async fn write(cli: &GitCli, repository: &Repository) -> Result<String, VcsError> {
    let out = repo_op(cli, Operation::WriteTree, repository, &["write-tree"], None).await?;
    Ok(out.text.trim().to_string())
}

/// `Git.tree.capture`: refresh every scope then write the index as a tree, under the git-dir lock.
pub async fn capture(
    cli: &GitCli,
    repository: &Repository,
    scopes: &[String],
    ignores: Option<&Repository>,
    maximum_untracked_bytes: Option<u64>,
) -> Result<String, VcsError> {
    let lock = cli.lock(&repository.git_dir);
    let _guard = lock.lock().await;
    for scope in scopes {
        refresh(cli, repository, scope, ignores, maximum_untracked_bytes).await?;
    }
    write(cli, repository).await
}

/// `Git.tree.files`: paths changed between two trees.
pub async fn files(
    cli: &GitCli,
    repository: &Repository,
    from: &str,
    to: &str,
) -> Result<Vec<String>, VcsError> {
    let args = ["diff", "--name-only", "-z", from, to];
    let out = repo_op(cli, Operation::ListFiles, repository, &args, None).await?;
    Ok(nul_split(&out.text))
}

/// `Git.tree.diff`: per-file status, numstat and unified patch (empty for binary files).
pub async fn diff(
    cli: &GitCli,
    repository: &Repository,
    from: &str,
    to: &str,
    context: Option<u32>,
    paths: Option<&[String]>,
) -> Result<Vec<FileDiff>, VcsError> {
    let paths = match paths {
        Some(paths) => paths.to_vec(),
        None => files(cli, repository, from, to).await?,
    };
    let unified = format!("--unified={}", context.unwrap_or(3));
    let mut out = Vec::with_capacity(paths.len());
    for file in paths {
        let run = |mode: &str| {
            let args = [
                "diff".to_string(),
                mode.to_string(),
                "--no-renames".to_string(),
                from.to_string(),
                to.to_string(),
                "--".to_string(),
                file.clone(),
            ];
            async move { repo_op(cli, Operation::Diff, repository, &args, None).await }
        };
        let status_text = run("--name-status").await?.text;
        let status = match status_text.trim().chars().next() {
            Some('A') => DiffStatus::Added,
            Some('D') => DiffStatus::Deleted,
            _ => DiffStatus::Modified,
        };
        let stats_text = run("--numstat").await?.text;
        let stats: Vec<&str> = stats_text.split('\t').collect();
        let binary = stats.first() == Some(&"-") || stats.get(1) == Some(&"-");
        let patch = if binary {
            String::new()
        } else {
            run(&unified).await?.text
        };
        let count = |index: usize| -> u64 {
            if binary {
                return 0;
            }
            stats
                .get(index)
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0)
        };
        out.push(FileDiff {
            file: Some(file.clone()),
            patch: Some(patch),
            additions: count(0),
            deletions: count(1),
            status: Some(status),
        });
    }
    Ok(out)
}
