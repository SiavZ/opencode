//! Purpose: live watcher tests for `NativeWatcher` on the host backend (FSEvents/inotify/...):
//! add/change/unlink, per-path coalescing, ignore patterns, single Overflow on a full queue, and
//! close() stopping delivery. Mirrors `packages/core/test/filesystem/watcher.test.ts` semantics.
//! Scope: live filesystem tests (tempfile), timing tolerant.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/watcher.rs
//! `-- platform-native/src/fs/mod.rs
//! External:
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! |-- tokio::test
//! |-- tokio::time::Instant::now
//! |-- tokio::time::sleep
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: every test waits for watcher readiness with a probe file before asserting.
//! Contract tests: platform-native/tests/fs_watcher.rs

use opencode_platform_api::watcher::{WatchEvent, WatchHandle, WatchKind, WatchOptions, Watcher};
use opencode_platform_native::fs::NativeWatcher;
use std::path::{Path, PathBuf};
use std::time::Duration;

async fn start(options: WatchOptions) -> (tempfile::TempDir, PathBuf, Box<dyn WatchHandle>) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let handle = NativeWatcher::new()
        .watch(root.clone(), options)
        .await
        .unwrap();
    ready(&root, handle.as_ref()).await;
    (dir, root, handle)
}

/// Like the baseline `ready`: keep touching a probe until the backend reports it.
async fn ready(root: &Path, handle: &dyn WatchHandle) {
    let probe = root.join(".watcher-ready");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut attempt = 0;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "watcher never became ready"
        );
        attempt += 1;
        std::fs::write(&probe, format!("ready-{attempt}")).unwrap();
        if let Ok(Some(WatchEvent::Changed { path, .. })) =
            tokio::time::timeout(Duration::from_millis(500), handle.next()).await
            && path == probe
        {
            break;
        }
    }
    std::fs::remove_file(&probe).unwrap();
    drain(handle, Duration::from_millis(400)).await;
}

async fn drain(handle: &dyn WatchHandle, quiet: Duration) -> Vec<WatchEvent> {
    let mut out = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(quiet, handle.next()).await {
        out.push(event);
    }
    out
}

async fn next_for(handle: &dyn WatchHandle, path: &Path) -> WatchKind {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, handle.next()).await {
            Ok(Some(WatchEvent::Changed { path: seen, kind })) if seen == path => return kind,
            Ok(Some(_)) => {}
            other => panic!("no event for {}: {other:?}", path.display()),
        }
    }
}

fn options(coalesce: u64) -> WatchOptions {
    WatchOptions {
        coalesce: Duration::from_millis(coalesce),
        ..WatchOptions::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishes_root_create_update_and_delete_events() {
    let (_dir, root, handle) = start(options(50)).await;
    let file = root.join("watch.txt");
    std::fs::write(&file, "a").unwrap();
    assert_eq!(next_for(handle.as_ref(), &file).await, WatchKind::Add);
    drain(handle.as_ref(), Duration::from_millis(300)).await;
    std::fs::write(&file, "b").unwrap();
    let change = next_for(handle.as_ref(), &file).await;
    // FSEvents may re-report the historical create flag with a later modify (recorded behavior).
    assert!(
        matches!(change, WatchKind::Change | WatchKind::Add),
        "{change:?}"
    );
    drain(handle.as_ref(), Duration::from_millis(300)).await;
    std::fs::remove_file(&file).unwrap();
    let mut kinds = vec![next_for(handle.as_ref(), &file).await];
    kinds.extend(
        drain(handle.as_ref(), Duration::from_millis(300))
            .await
            .into_iter()
            .filter_map(|event| match event {
                WatchEvent::Changed { path, kind } if path == file => Some(kind),
                _ => None,
            }),
    );
    assert_eq!(kinds.last(), Some(&WatchKind::Unlink), "{kinds:?}");
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coalesces_repeated_events_for_one_path_within_the_window() {
    let (_dir, root, handle) = start(options(400)).await;
    let file = root.join("burst.txt");
    for index in 0..20 {
        std::fs::write(&file, format!("v{index}")).unwrap();
    }
    let events = drain(handle.as_ref(), Duration::from_millis(900)).await;
    let for_file: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, WatchEvent::Changed { path, .. } if *path == file))
        .collect();
    assert_eq!(
        for_file,
        vec![&WatchEvent::Changed {
            path: file.clone(),
            kind: WatchKind::Add
        }],
        "{events:?}"
    );
    // Create and delete inside one window cancel out.
    let transient = root.join("transient.txt");
    std::fs::write(&transient, "x").unwrap();
    std::fs::remove_file(&transient).unwrap();
    let events = drain(handle.as_ref(), Duration::from_millis(900)).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, WatchEvent::Changed { path, .. } if *path == transient)),
        "{events:?}"
    );
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn honors_ignore_prefixes_and_globs() {
    let (_dir, root, handle) = start(WatchOptions {
        ignore: vec!["skipped".into(), "**/*.log".into()],
        ..options(50)
    })
    .await;
    std::fs::create_dir(root.join("skipped")).unwrap();
    std::fs::write(root.join("skipped/inner.txt"), "x").unwrap();
    std::fs::write(root.join("noise.log"), "x").unwrap();
    let kept = root.join("kept.txt");
    std::fs::write(&kept, "x").unwrap();
    let events = drain(handle.as_ref(), Duration::from_millis(800)).await;
    let paths: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            WatchEvent::Changed { path, .. } => Some(path.clone()),
            WatchEvent::Overflow => None,
        })
        .collect();
    assert!(paths.contains(&kept), "{paths:?}");
    assert!(
        paths
            .iter()
            .all(|path| !path.starts_with(root.join("skipped"))
                && path.extension().is_none_or(|ext| ext != "log")),
        "{paths:?}"
    );
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivers_exactly_one_overflow_when_the_queue_fills() {
    let (_dir, root, handle) = start(WatchOptions {
        capacity: 2,
        ..options(10)
    })
    .await;
    for index in 0..200 {
        std::fs::write(root.join(format!("f{index}.txt")), "x").unwrap();
    }
    // Do not consume while the backend floods the bounded queues.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let events = drain(handle.as_ref(), Duration::from_millis(500)).await;
    let overflows = events
        .iter()
        .filter(|event| **event == WatchEvent::Overflow)
        .count();
    assert_eq!(overflows, 1, "{events:?}");
    assert!(
        events.len() <= 3,
        "at most capacity events precede the marker: {events:?}"
    );
    assert_eq!(events.last(), Some(&WatchEvent::Overflow));
    // The stream keeps working after overflow.
    let after = root.join("after.txt");
    std::fs::write(&after, "x").unwrap();
    assert_eq!(next_for(handle.as_ref(), &after).await, WatchKind::Add);
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_joins_the_task_and_stops_publishing() {
    let (_dir, root, handle) = start(options(20)).await;
    tokio::time::timeout(Duration::from_secs(5), handle.close())
        .await
        .expect("close joins promptly");
    std::fs::write(root.join("after-close.txt"), "gone").unwrap();
    let next = tokio::time::timeout(Duration::from_millis(500), handle.next()).await;
    assert!(matches!(next, Ok(None)), "{next:?}");
}

#[tokio::test]
async fn watching_a_missing_root_fails_with_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let error = NativeWatcher::new()
        .watch(dir.path().join("missing"), WatchOptions::default())
        .await
        .unwrap_err();
    assert!(error.is_not_found(), "{error:?}");
}
