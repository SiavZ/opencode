//! Purpose: real temp-dir conformance tests for `NativeFileMutation`, ported from
//! `packages/core/test/file-mutation.test.ts` plus BOM, create-exists, concurrency and FIFO-based
//! lock serialization checks.
//! Scope: live filesystem tests (tempfile), no fixtures.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/location.rs
//! `-- platform-native/src/fs/mutation.rs
//! External:
//! |-- tempfile::TempDir
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: every test owns its temp dirs and never touches user data.
//! Contract tests: platform-native/tests/fs_mutation.rs

use opencode_platform_api::fs::{
    FileMutation, LocationMutation, MutationError, MutationOperation, MutationResult, ResolveInput,
    Target,
};
use opencode_platform_native::fs::{NativeFileMutation, NativeLocationMutation};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

async fn setup() -> (
    tempfile::TempDir,
    NativeLocationMutation,
    NativeFileMutation,
) {
    let dir = tempfile::tempdir().unwrap();
    let location = NativeLocationMutation::new(dir.path()).await.unwrap();
    (dir, location, NativeFileMutation::new())
}

async fn resolve(location: &NativeLocationMutation, path: impl AsRef<Path>) -> Target {
    location
        .resolve(ResolveInput {
            path: path.as_ref().to_string_lossy().into_owned(),
            kind: None,
        })
        .await
        .unwrap()
}

fn expect(target: &Target, operation: MutationOperation, existed: bool) -> MutationResult {
    MutationResult {
        operation,
        target: target.canonical.clone(),
        resource: target.resource.clone(),
        existed,
    }
}

#[tokio::test]
async fn writes_an_existing_internal_file_and_returns_a_stable_result() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("hello.txt"), "before").unwrap();
    let target = resolve(&location, "hello.txt").await;
    let result = files.write(&target, b"after").await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Write, true));
    assert_eq!(result.resource, "hello.txt");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
        "after"
    );
}

#[tokio::test]
async fn writes_a_prospective_internal_file_and_creates_parent_directories() {
    let (_dir, location, files) = setup().await;
    let target = resolve(&location, Path::new("src").join("nested").join("hello.txt")).await;
    let result = files.write(&target, b"hello").await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Write, false));
    assert_eq!(result.resource, "src/nested/hello.txt");
    assert_eq!(std::fs::read_to_string(&result.target).unwrap(), "hello");
}

#[tokio::test]
async fn preserves_exactly_one_bom_for_text_writes_and_normalizes_created_text() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("preserved.txt"), "\u{feff}before").unwrap();
    std::fs::write(dir.path().join("kept.txt"), "\u{feff}before").unwrap();
    std::fs::write(dir.path().join("plain.txt"), "before").unwrap();
    let preserved = resolve(&location, "preserved.txt").await;
    let kept = resolve(&location, "kept.txt").await;
    let plain = resolve(&location, "plain.txt").await;
    let created = resolve(&location, "created.txt").await;

    let result = files
        .write_text_preserving_bom(&preserved, "\u{feff}after")
        .await
        .unwrap();
    assert!(result.existed);
    // Existing BOM is retained even when the new text has none.
    files
        .write_text_preserving_bom(&kept, "after")
        .await
        .unwrap();
    files
        .write_text_preserving_bom(&plain, "after")
        .await
        .unwrap();
    let result = files
        .write_text_preserving_bom(&created, "\u{feff}\u{feff}\u{feff}created")
        .await
        .unwrap();
    assert!(!result.existed);

    let read = |name: &str| std::fs::read(dir.path().join(name)).unwrap();
    assert_eq!(read("preserved.txt"), "\u{feff}after".as_bytes());
    assert_eq!(read("kept.txt"), "\u{feff}after".as_bytes());
    assert_eq!(read("plain.txt"), b"after");
    assert_eq!(read("created.txt"), "\u{feff}created".as_bytes());
}

#[tokio::test]
async fn rejects_create_when_a_prospective_target_appears_after_resolution() {
    let (dir, location, files) = setup().await;
    let target = resolve(&location, "appeared.txt").await;
    std::fs::write(dir.path().join("appeared.txt"), "winner").unwrap();
    let error = files.create(&target, b"replacement").await.unwrap_err();
    assert_eq!(
        error,
        MutationError::TargetExists {
            path: target.canonical.to_string_lossy().into_owned()
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("appeared.txt")).unwrap(),
        "winner"
    );
}

#[tokio::test]
async fn create_makes_missing_parents() {
    let (dir, location, files) = setup().await;
    let target = resolve(&location, "a/b/c.txt").await;
    let result = files.create(&target, b"x").await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Write, false));
    assert_eq!(std::fs::read(dir.path().join("a/b/c.txt")).unwrap(), b"x");
}

#[tokio::test]
async fn creates_when_an_existing_target_disappears_after_resolution() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("removed.txt"), "before").unwrap();
    let target = resolve(&location, "removed.txt").await;
    std::fs::remove_file(dir.path().join("removed.txt")).unwrap();
    let result = files.create(&target, b"after").await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Write, false));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("removed.txt")).unwrap(),
        "after"
    );
}

#[tokio::test]
async fn removes_an_existing_internal_file() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("remove.txt"), "remove").unwrap();
    let target = resolve(&location, "remove.txt").await;
    let result = files.remove(&target).await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Remove, true));
    assert!(!dir.path().join("remove.txt").exists());
}

#[tokio::test]
async fn reports_a_missing_target_as_not_removed() {
    let (_dir, location, files) = setup().await;
    let target = resolve(&location, "missing.txt").await;
    let result = files.remove(&target).await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Remove, false));
    assert_eq!(result.resource, "missing.txt");
}

#[tokio::test]
async fn remove_rejects_directories_like_node_rm_without_recursive() {
    let (dir, location, files) = setup().await;
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    let target = resolve(&location, "empty").await;
    let error = files.remove(&target).await.unwrap_err();
    assert!(matches!(error, MutationError::Platform(_)), "{error:?}");
    assert!(dir.path().join("empty").is_dir());
}

#[tokio::test]
async fn writes_and_removes_an_explicitly_resolved_external_target() {
    let (_dir, location, files) = setup().await;
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("external.txt");
    let target = resolve(&location, &path).await;
    assert!(target.external_directory.is_some());
    let result = files.write(&target, b"external").await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Write, false));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    let result = files.remove(&target).await.unwrap();
    assert_eq!(result, expect(&target, MutationOperation::Remove, true));
    assert!(!path.exists());
}

#[tokio::test]
async fn rejects_a_conditional_write_when_target_content_is_already_stale() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("stale.txt"), "current").unwrap();
    let target = resolve(&location, "stale.txt").await;
    let error = files
        .write_if_unchanged(&target, b"replacement", b"older")
        .await
        .unwrap_err();
    assert_eq!(
        error,
        MutationError::StaleContent {
            path: target.canonical.to_string_lossy().into_owned()
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("stale.txt")).unwrap(),
        "current"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allows_only_one_concurrent_conditional_write_based_on_the_same_bytes() {
    let (dir, location, files) = setup().await;
    std::fs::write(dir.path().join("shared.txt"), "initial").unwrap();
    let target = Arc::new(resolve(&location, "shared.txt").await);
    let files = Arc::new(files);
    let tasks: Vec<_> = (0..16)
        .map(|index| {
            let (files, target) = (files.clone(), target.clone());
            tokio::spawn(async move {
                let content = format!("writer-{index}");
                files
                    .write_if_unchanged(&target, content.as_bytes(), b"initial")
                    .await
                    .map(|_| content)
            })
        })
        .collect();
    let mut winners = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            Ok(content) => winners.push(content),
            Err(MutationError::StaleContent { .. }) => {}
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one conditional write wins");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("shared.txt")).unwrap(),
        winners[0]
    );
    assert_eq!(files.lock_entries(), 0, "lock entries are released");
}

/// A FIFO target blocks `open(O_WRONLY)` until a reader appears, so a write holds the target lock
/// for as long as the test wants. This observes real lock serialization without instrumentation.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serializes_writes_per_canonical_target_and_lets_distinct_targets_proceed() {
    let (dir, location, files) = setup().await;
    let fifo = dir.path().join("shared.fifo");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let first_plan = resolve(&location, "shared.fifo").await;
    let second_plan = resolve(&location, "./shared.fifo").await;
    let other = resolve(&location, "other.txt").await;
    let files = Arc::new(files);

    let first = tokio::spawn({
        let (files, target) = (files.clone(), first_plan.clone());
        async move { files.write(&target, b"first").await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = tokio::spawn({
        let (files, target) = (files.clone(), second_plan.clone());
        async move { files.write(&target, b"second").await }
    });
    // A different canonical target is not blocked by the held lock.
    tokio::time::timeout(Duration::from_secs(5), files.write(&other, b"other"))
        .await
        .expect("distinct target proceeds")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!first.is_finished(), "first write is blocked on the FIFO");
    assert!(
        !second.is_finished(),
        "second write waits for the target lock"
    );

    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            let first = std::fs::read(&fifo).unwrap();
            let second = std::fs::read(&fifo).unwrap();
            (first, second)
        })
    };
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let (first_bytes, second_bytes) = reader.join().unwrap();
    assert_eq!(first_bytes, b"first", "first holder writes first");
    assert_eq!(
        second_bytes, b"second",
        "second write ran only after the first released"
    );
    assert_eq!(files.lock_entries(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_caller_does_not_interrupt_a_started_write() {
    let (dir, location, files) = setup().await;
    let target = resolve(&location, "big.bin").await;
    let content = vec![b'x'; 8 << 20];
    let write = files.write(&target, &content);
    // Drop the future quickly; the spawned body still completes or never started.
    let _ = tokio::time::timeout(Duration::from_micros(50), write).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let on_disk = std::fs::read(dir.path().join("big.bin")).unwrap_or_default();
    assert!(
        on_disk.is_empty() || on_disk.len() == content.len(),
        "no torn write: {}",
        on_disk.len()
    );
}
