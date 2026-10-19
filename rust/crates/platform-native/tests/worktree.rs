//! Purpose: live tests for the worktree service: naming, create/populate, list, remove (branch
//! deletion and stale directories) and reset to the default branch.
//! Scope: `Worktrees` over real temp repos with isolated git config. Live.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/src/vcs/worktree.rs
//! `-- platform-native/tests/git_support.rs
//! External:
//! |-- serde_json::to_value
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: the worktree data root is a temp dir, never the real opencode data dir.
//! Contract tests: platform-native/tests/worktree.rs

mod git_support;

use git_support::{Fixture, read, write};
use opencode_platform_native::vcs::worktree::{failed_removes, slugify};
use opencode_platform_native::vcs::{WorktreeError, Worktrees};
use std::path::{Path, PathBuf};

fn service(fx: &Fixture, project: &Path) -> Worktrees {
    Worktrees {
        cli: fx.cli(),
        data_dir: fx.root().join("data"),
        project_id: "pid".into(),
        worktree: project.to_path_buf(),
        primary: project.to_path_buf(),
        vcs_git: true,
    }
}

#[test]
fn slugify_and_failed_removes_match_baseline() {
    assert_eq!(slugify("  My Feature!! Branch "), "my-feature-branch");
    assert_eq!(slugify("--x--"), "x");
    assert_eq!(
        failed_removes(&["warning: failed to remove 'node_modules/x': Permission denied\nother"]),
        vec!["node_modules/x"]
    );
}

#[tokio::test]
async fn creates_lists_and_removes_named_worktree() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    let svc = service(&fx, &project);

    let info = svc.make_info(Some("My Task"), false).await.unwrap();
    assert_eq!(info.name, "my-task");
    assert_eq!(info.branch.as_deref(), Some("opencode/my-task"));
    let expected = fx.root().join("data/worktree/pid/my-task");
    assert_eq!(PathBuf::from(&info.directory), expected);
    assert_eq!(svc.create_from_info(&info).await.unwrap(), None);
    assert_eq!(read(&expected.join("a.txt")), "a\n");

    // A taken name falls back to `<name>-<slug>`.
    let next = svc.make_info(Some("my task"), false).await.unwrap();
    assert!(next.name.starts_with("my-task-"), "{}", next.name);
    let generated = svc.make_info(None, true).await.unwrap();
    assert_eq!(generated.branch, None);
    assert_eq!(generated.name.split('-').count(), 2);

    let list = svc.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "my-task");
    assert_eq!(list[0].branch.as_deref(), Some("opencode/my-task"));
    let wire = serde_json::to_value(&list[0]).unwrap();
    assert_eq!(wire["branch"], "opencode/my-task");

    write(&expected.join("dirty.txt"), "x\n");
    assert!(svc.remove(&expected).await.unwrap());
    assert!(!expected.exists());
    let branches = fx.git(&project, &["branch", "--list", "opencode/*"]);
    assert!(branches.trim().is_empty(), "{branches}");
    assert!(svc.list().await.unwrap().is_empty());

    // Unknown directories are just deleted.
    let stray = fx.root().join("data/worktree/pid/stray");
    write(&stray.join("f"), "x");
    assert!(svc.remove(&stray).await.unwrap());
    assert!(!stray.exists());
}

#[tokio::test]
async fn reset_returns_worktree_to_default_branch() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    let svc = service(&fx, &project);
    let base = svc.default_branch(&project).await.unwrap();
    assert_eq!(
        (base.name.as_str(), base.reference.as_str()),
        ("main", "main")
    );

    let info = svc.make_info(Some("work"), false).await.unwrap();
    svc.create_from_info(&info).await.unwrap();
    let dir = PathBuf::from(&info.directory);
    write(&dir.join("a.txt"), "edited\n");
    write(&dir.join("junk/untracked.txt"), "junk\n");
    fx.git(&dir, &["commit", "-q", "-am", "diverge"]);

    assert!(svc.reset(&dir).await.unwrap());
    assert_eq!(read(&dir.join("a.txt")), "a\n");
    assert!(!dir.join("junk").exists());

    let error = svc.reset(&project).await.unwrap_err();
    assert_eq!(
        error,
        WorktreeError::ResetFailed("Cannot reset the primary workspace".into())
    );
    let missing = svc.reset(&fx.root().join("nope")).await.unwrap_err();
    assert_eq!(
        missing,
        WorktreeError::ResetFailed("Worktree not found".into())
    );
}

#[tokio::test]
async fn non_git_projects_are_rejected() {
    let fx = Fixture::new();
    let plain = fx.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let svc = Worktrees {
        vcs_git: false,
        ..service(&fx, &plain)
    };
    assert!(matches!(
        svc.make_info(None, false).await,
        Err(WorktreeError::NotGit(_))
    ));
    assert!(svc.list().await.unwrap().is_empty());
    assert!(!fx.root().join("data").exists());
}
