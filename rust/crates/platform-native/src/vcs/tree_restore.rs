//! Purpose: selective tree restore operations of packages/core/src/git.ts (`tree.preview`,
//! `tree.restore`, `tree.checkout`) and the `ls-tree` entry lookup they share.
//! Scope: free functions over `GitCli`, each holding the git-dir lock for its whole duration.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/error.rs
//! |-- platform-native/src/vcs/run.rs
//! `-- platform-native/src/vcs/tree.rs
//! External:
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- platform-native/src/vcs/snapshot.rs
//! `-- platform-native/src/vcs/tree.rs
//! Invariants: preview never writes the shared index (it uses a throwaway GIT_INDEX_FILE that is
//! always deleted); restore removes paths absent from their tree; checkout leaves files absent
//! from the tree untouched.
//! Contract tests: platform-native/tests/snapshot.rs

use super::error::{Operation, VcsError};
use super::run::GitCli;
use super::tree::{diff, repo_op, repo_op_env};
use opencode_platform_api::git::Repository;
use opencode_platform_api::snapshot::FileDiff;
use std::collections::BTreeMap;

struct Entry {
    mode: String,
    object: String,
}

/// `ls-tree -z <tree> -- <file>` parsed into mode and object; None when absent from the tree.
async fn entry(
    cli: &GitCli,
    repository: &Repository,
    tree: &str,
    file: &str,
) -> Result<Option<Entry>, VcsError> {
    let args = ["ls-tree", "-z", tree, "--", file];
    let out = repo_op(cli, Operation::Restore, repository, &args, None).await?;
    let text = out.text.strip_suffix('\0').unwrap_or(&out.text);
    if text.is_empty() {
        return Ok(None);
    }
    let invalid =
        || VcsError::operation(Operation::Restore, format!("Invalid tree entry for {file}"));
    let (meta, _) = text.split_once('\t').ok_or_else(invalid)?;
    let mut parts = meta.split_whitespace();
    let (Some(mode), Some(_kind), Some(object)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid());
    };
    if !mode.chars().all(|c| c.is_ascii_digit()) || !object.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    Ok(Some(Entry {
        mode: mode.to_string(),
        object: object.to_string(),
    }))
}

/// `Git.tree.preview`: build the restored tree in a throwaway index and diff it against `current`.
pub async fn preview(
    cli: &GitCli,
    repository: &Repository,
    current: &str,
    files: &BTreeMap<String, String>,
    context: Option<u32>,
) -> Result<Vec<FileDiff>, VcsError> {
    let lock = cli.lock(&repository.git_dir);
    let _guard = lock.lock().await;
    let index = repository
        .git_dir
        .join(format!("preview-{}.index", uuid::Uuid::new_v4()));
    let result = preview_with_index(cli, repository, current, files, context, &index).await;
    let _ = tokio::fs::remove_file(&index).await;
    result
}

async fn preview_with_index(
    cli: &GitCli,
    repository: &Repository,
    current: &str,
    files: &BTreeMap<String, String>,
    context: Option<u32>,
    index: &std::path::Path,
) -> Result<Vec<FileDiff>, VcsError> {
    let env = [("GIT_INDEX_FILE", index.to_string_lossy().into_owned())];
    let op = Operation::Diff;
    repo_op_env(cli, op, repository, &["read-tree", current], None, &env).await?;
    for (file, tree) in files {
        let source = entry(cli, repository, tree, file).await?;
        let args: Vec<&str> = match &source {
            None => vec!["update-index", "--force-remove", "--", file],
            Some(source) => vec![
                "update-index",
                "--add",
                "--cacheinfo",
                &source.mode,
                &source.object,
                file,
            ],
        };
        repo_op_env(cli, op, repository, &args, None, &env).await?;
    }
    let target = repo_op_env(cli, op, repository, &["write-tree"], None, &env).await?;
    let paths: Vec<String> = files.keys().cloned().collect();
    diff(
        cli,
        repository,
        current,
        target.text.trim(),
        context,
        Some(&paths),
    )
    .await
}

/// `Git.tree.restore`: check each path out of its tree, deleting it when the tree lacks it.
pub async fn restore(
    cli: &GitCli,
    repository: &Repository,
    files: &BTreeMap<String, String>,
) -> Result<(), VcsError> {
    let lock = cli.lock(&repository.git_dir);
    let _guard = lock.lock().await;
    for (file, tree) in files {
        if entry(cli, repository, tree, file).await?.is_some() {
            let args = ["checkout", tree.as_str(), "--", file.as_str()];
            repo_op(cli, Operation::Restore, repository, &args, None).await?;
            continue;
        }
        remove_path(&repository.root.join(file))
            .await
            .map_err(|_| {
                VcsError::operation(Operation::Restore, format!("Failed to remove {file}"))
            })?;
    }
    Ok(())
}

/// `fs.remove(path, { recursive: true, force: true })`.
pub async fn remove_path(path: &std::path::Path) -> std::io::Result<()> {
    let result = match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(path).await,
        Ok(_) => tokio::fs::remove_file(path).await,
    };
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `Git.tree.checkout`: replace the index with `tree` and force-check out every entry.
pub async fn checkout(cli: &GitCli, repository: &Repository, tree: &str) -> Result<(), VcsError> {
    let lock = cli.lock(&repository.git_dir);
    let _guard = lock.lock().await;
    repo_op(
        cli,
        Operation::Restore,
        repository,
        &["read-tree", tree],
        None,
    )
    .await?;
    let args = ["checkout-index", "--all", "--force"];
    repo_op(cli, Operation::Restore, repository, &args, None).await?;
    Ok(())
}
