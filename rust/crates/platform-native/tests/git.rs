//! Purpose: live tests for the native `Git` adapter: discovery, history queries, worktree
//! create/list/remove through the trait, tree diffs and working-tree change sets.
//! Scope: real git in temp repos with isolated config. Live.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/git.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/change.rs
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/src/vcs/tree.rs
//! `-- platform-native/tests/git_support.rs
//! External:
//! |-- change::apply
//! |-- change::capture
//! |-- change::discard
//! |-- serde_json::to_value
//! |-- tokio::test
//! `-- tree::files
//! Used by:
//! `-- none, entrypoint
//! Invariants: no test touches the user's git config or data dirs.
//! Contract tests: platform-native/tests/git.rs

mod git_support;

use git_support::{Fixture, read, write};
use opencode_platform_api::git::{Git, GitError};
use opencode_platform_api::snapshot::DiffStatus;
use opencode_platform_native::vcs::change::{self, IndexMode};
use opencode_platform_native::vcs::{NativeGit, VcsError, tree};

#[tokio::test]
async fn discovers_repository_and_history() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    std::fs::create_dir_all(project.join("nested/deep")).unwrap();
    let git = NativeGit::new(fx.cli());

    let repo = git
        .discover(&project.join("nested/deep"))
        .await
        .expect("repo");
    assert_eq!(repo.root, project);
    assert_eq!(repo.git_dir, project.join(".git"));
    assert_eq!(repo.common_dir, project.join(".git"));

    let head = git.head(&repo).await.expect("head");
    assert_eq!(head.len(), 40);
    assert_eq!(git.branch(&repo).await.as_deref(), Some("main"));
    assert_eq!(git.root_commits(&repo).await, vec![head.clone()]);
    assert_eq!(git.remote_url(&repo, None).await, None);
    fx.git(
        &project,
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );
    assert_eq!(
        git.remote_url(&repo, Some("origin")).await.as_deref(),
        Some("https://example.test/repo.git")
    );
    fx.git(&project, &["checkout", "-q", "--detach"]);
    assert_eq!(git.branch(&repo).await, None);

    let plain = fx.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(git.discover(&plain).await, None);
}

#[tokio::test]
async fn creates_lists_and_removes_worktrees() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    let git = NativeGit::new(fx.cli());
    let repo = git.discover(&project).await.unwrap();

    let linked = fx.root().join("linked");
    let created = git
        .worktree_create(&repo, &linked, "feature")
        .await
        .unwrap();
    assert_eq!(created.root, linked);
    assert_eq!(created.common_dir, project.join(".git"));
    assert_ne!(created.git_dir, created.common_dir);
    let detached = fx.root().join("detached");
    git.worktree_create(&repo, &detached, "").await.unwrap();

    let list = git.worktree_list(&repo).await.unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].path, project);
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    let find = |path: &std::path::Path| list.iter().find(|w| w.path == path).unwrap();
    assert_eq!(find(&linked).branch.as_deref(), Some("feature"));
    assert_eq!(find(&detached).branch, None);
    assert!(find(&detached).head.is_some());

    write(&linked.join("dirty.txt"), "x\n");
    let error = git
        .remove_worktree(&repo, &linked, false)
        .await
        .unwrap_err();
    assert!(error.force_required(), "{error:?}");
    let error: GitError = error.into();
    assert!(matches!(error, GitError::Worktree(_)));
    git.worktree_remove(&repo, &linked, true).await.unwrap();
    git.worktree_remove(&repo, &detached, false).await.unwrap();
    assert!(!linked.exists());
    assert_eq!(git.worktree_list(&repo).await.unwrap().len(), 1);
}

#[tokio::test]
async fn diff_trees_between_commits() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let git = NativeGit::new(fx.cli());
    let repo = git.discover(&project).await.unwrap();
    let from = fx
        .git(&project, &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_string();
    write(&project.join("a.txt"), "a\nmore\n");
    std::fs::remove_file(project.join("b.txt")).unwrap();
    fx.git(&project, &["add", "-A"]);
    fx.git(&project, &["commit", "-q", "-m", "next"]);
    let to = fx
        .git(&project, &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_string();

    let diffs = git.diff_trees(&repo, &from, &to, Some(3)).await.unwrap();
    assert_eq!(diffs.len(), 2);
    assert_eq!(diffs[0].file.as_deref(), Some("a.txt"));
    assert_eq!(diffs[0].status, Some(DiffStatus::Modified));
    assert_eq!(diffs[0].additions, 1);
    assert_eq!(diffs[1].status, Some(DiffStatus::Deleted));
    let wire = serde_json::to_value(&diffs[1]).unwrap();
    assert_eq!(wire["status"], "deleted");
    assert_eq!(wire["file"], "b.txt");

    let error = git.diff_trees(&repo, "nope", &to, None).await.unwrap_err();
    assert!(matches!(error, GitError::Process(_)));
    let files = tree::files(git.cli(), &repo, &from, &to).await.unwrap();
    assert_eq!(files, vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn change_capture_apply_and_discard_round_trip() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("sub/a.txt", "a\n"), ("top.txt", "t\n")]);
    let git = NativeGit::new(fx.cli());
    let repo = git.discover(&project).await.unwrap();
    let sub = project.join("sub");
    write(&sub.join("a.txt"), "changed\n");
    write(&sub.join("new.txt"), "new\n");
    std::fs::write(sub.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    write(&project.join("top.txt"), "outside\n");

    let changes = change::capture(git.cli(), &repo, &sub).await.unwrap();
    assert!(changes.contains("sub/a.txt"));
    assert!(changes.contains("new.txt"));
    assert!(!changes.contains("top.txt"));

    change::discard(git.cli(), &repo, &sub, IndexMode::Reset, true)
        .await
        .unwrap();
    assert_eq!(read(&sub.join("a.txt")), "a\n");
    assert!(!sub.join("new.txt").exists());
    assert_eq!(read(&project.join("top.txt")), "outside\n");

    change::apply(git.cli(), &project, &changes).await.unwrap();
    assert_eq!(read(&sub.join("a.txt")), "changed\n");
    assert_eq!(read(&sub.join("new.txt")), "new\n");
    assert_eq!(
        std::fs::read(sub.join("bin.dat")).unwrap(),
        vec![0u8, 1, 2, 3]
    );

    let error = change::apply(git.cli(), &project, "garbage")
        .await
        .unwrap_err();
    assert!(matches!(error, VcsError::Patch { .. }));
}
