//! Purpose: live snapshot tests over real temp git repos, ported from
//! packages/core/test/snapshot.test.ts plus status, binary, ignore, size-limit, escape and gc cases.
//! Scope: `NativeSnapshots` through the `Snapshots` trait. Live (spawns real git).
//! Uses:
//! rust/crates/
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/repo.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! `-- platform-native/tests/git_support.rs
//! External:
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: every repo, data dir and HOME live under a temp dir.
//! Contract tests: platform-native/tests/snapshot.rs

mod git_support;

use git_support::{Fixture, read, write};
use opencode_platform_api::snapshot::{DiffStatus, SnapshotOperation, Snapshots};
use opencode_platform_native::vcs::snapshot::hash_fast;
use opencode_platform_native::vcs::{NativeGit, NativeSnapshots, SnapshotConfig};
use std::collections::BTreeMap;
use std::path::Path;

async fn open(fx: &Fixture, project: &Path, directory: &Path) -> NativeSnapshots {
    let config = SnapshotConfig {
        data_dir: fx.root().join("data"),
        project_id: "project-id".into(),
        project_directory: project.to_path_buf(),
        directory: directory.to_path_buf(),
        vcs_git: true,
        enabled: true,
    };
    NativeSnapshots::open(NativeGit::new(fx.cli()), config).await
}

#[tokio::test]
async fn captures_and_restores_location_scoped_changes() {
    let fx = Fixture::new();
    let project = fx.repo(
        "project",
        &[("scope/tracked.txt", "one\n"), ("outside.txt", "outside\n")],
    );
    let location = project.join("scope");
    let snapshot = open(&fx, &project, &location).await;
    let before = snapshot.capture().await.expect("before");

    write(&location.join("tracked.txt"), "two\n");
    write(&location.join("added.txt"), "added\n");
    write(&project.join("outside.txt"), "changed outside\n");
    let after = snapshot.capture().await.expect("after");

    assert_eq!(
        snapshot.files(&before, &after).await.unwrap(),
        vec!["scope/added.txt", "scope/tracked.txt"]
    );
    let plan = BTreeMap::from([("scope/tracked.txt".to_string(), before.clone())]);
    let preview = snapshot.preview(&plan, Some(1)).await.unwrap();
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].file.as_deref(), Some("scope/tracked.txt"));
    assert_eq!((preview[0].additions, preview[0].deletions), (1, 1));
    // Preview must not touch the worktree.
    assert_eq!(read(&location.join("tracked.txt")), "two\n");

    snapshot.restore(&plan).await.unwrap();
    assert_eq!(read(&location.join("tracked.txt")), "one\n");
    assert_eq!(read(&location.join("added.txt")), "added\n");
    assert_eq!(read(&project.join("outside.txt")), "changed outside\n");
}

#[tokio::test]
async fn treats_capture_outside_git_as_unavailable() {
    let fx = Fixture::new();
    let plain = fx.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let snapshot = open(&fx, &plain, &plain).await;
    assert_eq!(snapshot.capture().await, None);
    let error = snapshot.files("a", "b").await.unwrap_err();
    assert_eq!(error.operation, SnapshotOperation::Files);
    assert_eq!(error.message, "Project is not a Git repository");
    assert!(!fx.root().join("data").exists());
}

#[tokio::test]
async fn isolates_snapshot_indexes_by_canonical_git_worktree() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("tracked.txt", "main\n")]);
    let linked = fx.root().join("linked");
    fx.git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );

    assert!(
        open(&fx, &project, &project)
            .await
            .capture()
            .await
            .is_some()
    );
    assert!(open(&fx, &linked, &linked).await.capture().await.is_some());
    let base = fx.root().join("data/snapshot/project-id");
    assert!(
        base.join(hash_fast(&project.to_string_lossy()))
            .join("HEAD")
            .exists()
    );
    assert!(
        base.join(hash_fast(&linked.to_string_lossy()))
            .join("HEAD")
            .exists()
    );
}

#[tokio::test]
async fn checks_out_legacy_snapshot_without_removing_unrelated_files() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("tracked.txt", "one\n")]);
    let snapshot = open(&fx, &project, &project).await;
    let before = snapshot.capture().await.expect("before");
    write(&project.join("tracked.txt"), "two\n");
    write(&project.join("unrelated.txt"), "keep\n");
    snapshot.checkout(&before).await.unwrap();
    assert_eq!(read(&project.join("tracked.txt")), "one\n");
    assert_eq!(read(&project.join("unrelated.txt")), "keep\n");
}

#[tokio::test]
async fn diff_reports_added_deleted_modified_and_binary() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("keep.txt", "a\nb\n"), ("gone.txt", "bye\n")]);
    let snapshot = open(&fx, &project, &project).await;
    let before = snapshot.capture().await.unwrap();
    write(&project.join("keep.txt"), "a\nc\nd\n");
    std::fs::remove_file(project.join("gone.txt")).unwrap();
    write(&project.join("new.txt"), "hello\n");
    std::fs::write(project.join("blob.bin"), [0u8, 1, 2, 0, 255, 0]).unwrap();
    let after = snapshot.capture().await.unwrap();

    let diffs = snapshot.diff(&before, &after, None, None).await.unwrap();
    let by = |name: &str| {
        diffs
            .iter()
            .find(|d| d.file.as_deref() == Some(name))
            .unwrap()
    };
    assert_eq!(diffs.len(), 4);
    let keep = by("keep.txt");
    assert_eq!(keep.status, Some(DiffStatus::Modified));
    assert_eq!((keep.additions, keep.deletions), (2, 1));
    assert!(keep.patch.as_deref().unwrap().contains("+c\n+d\n"));
    let gone = by("gone.txt");
    assert_eq!(
        (gone.status, gone.deletions),
        (Some(DiffStatus::Deleted), 1)
    );
    let new = by("new.txt");
    assert_eq!((new.status, new.additions), (Some(DiffStatus::Added), 1));
    let blob = by("blob.bin");
    assert_eq!(blob.status, Some(DiffStatus::Added));
    assert_eq!((blob.additions, blob.deletions), (0, 0));
    assert_eq!(blob.patch.as_deref(), Some(""));

    let only = ["keep.txt".to_string()];
    let selected = snapshot
        .diff(&before, &after, Some(0), Some(&only))
        .await
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert!(!selected[0].patch.as_deref().unwrap().contains("\n a\n"));

    // Restoring a path absent from its tree removes it; a deleted path comes back.
    let plan = BTreeMap::from([
        ("new.txt".to_string(), before.clone()),
        ("gone.txt".to_string(), before.clone()),
    ]);
    let preview = snapshot.preview(&plan, None).await.unwrap();
    let statuses: Vec<_> = preview
        .iter()
        .map(|d| (d.file.clone().unwrap(), d.status))
        .collect();
    assert!(statuses.contains(&("new.txt".into(), Some(DiffStatus::Deleted))));
    assert!(statuses.contains(&("gone.txt".into(), Some(DiffStatus::Added))));
    snapshot.restore(&plan).await.unwrap();
    assert!(!project.join("new.txt").exists());
    assert_eq!(read(&project.join("gone.txt")), "bye\n");
}

#[tokio::test]
async fn skips_ignored_and_oversized_untracked_files() {
    let fx = Fixture::new();
    let project = fx.repo(
        "project",
        &[(".gitignore", "ignored.log\n"), ("a.txt", "a\n")],
    );
    let snapshot = open(&fx, &project, &project).await;
    let before = snapshot.capture().await.unwrap();
    write(&project.join("ignored.log"), "noise\n");
    let big = vec![b'x'; 2 * 1024 * 1024 + 1];
    std::fs::write(project.join("big.dat"), &big).unwrap();
    write(&project.join("small.txt"), "ok\n");
    let after = snapshot.capture().await.unwrap();
    assert_eq!(
        snapshot.files(&before, &after).await.unwrap(),
        vec!["small.txt"]
    );
}

#[tokio::test]
async fn rejects_escaping_paths_and_disabled_snapshots() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    let snapshot = open(&fx, &project, &project).await;
    let tree = snapshot.capture().await.unwrap();
    let plan = BTreeMap::from([("../evil.txt".to_string(), tree.clone())]);
    let error = snapshot.restore(&plan).await.unwrap_err();
    assert_eq!(error.message, "Path escapes the project: ../evil.txt");
    let error = snapshot.preview(&plan, None).await.unwrap_err();
    assert_eq!(error.operation, SnapshotOperation::Preview);

    let config = SnapshotConfig {
        data_dir: fx.root().join("data"),
        project_id: "project-id".into(),
        project_directory: project.clone(),
        directory: project.clone(),
        vcs_git: true,
        enabled: false,
    };
    let disabled = NativeSnapshots::open(NativeGit::new(fx.cli()), config).await;
    assert_eq!(disabled.capture().await, None);
    let error = disabled.restore(&BTreeMap::new()).await.unwrap_err();
    assert_eq!(error.message, "Snapshots are disabled");
}

#[tokio::test]
async fn cleanup_runs_gc_on_the_snapshot_git_dir() {
    let fx = Fixture::new();
    let project = fx.repo("project", &[("a.txt", "a\n")]);
    let snapshot = open(&fx, &project, &project).await;
    let first = snapshot.capture().await.unwrap();
    snapshot.cleanup().await;
    // Objects referenced by the index survive gc; the snapshot stays usable.
    write(&project.join("a.txt"), "b\n");
    let second = snapshot.capture().await.unwrap();
    assert_eq!(
        snapshot.files(&first, &second).await.unwrap(),
        vec!["a.txt"]
    );
    let alternates = snapshot.git_dir().join("objects/info/alternates");
    assert!(read(&alternates).trim_end().ends_with("objects"));
}
