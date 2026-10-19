//! Purpose: real temp-dir conformance tests for `NativeLocationMutation`, ported from
//! `packages/core/test/location-mutation.test.ts` plus non-directory ancestors, missing ancestor
//! walks, directory-kind external boundaries and observed macOS case-insensitivity.
//! Scope: live filesystem tests (tempfile), no fixtures.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/location.rs
//! `-- platform-native/src/fs/mutation.rs
//! External:
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: every test owns its temp dirs and never touches user data.
//! Contract tests: platform-native/tests/fs_location.rs

use opencode_platform_api::PlatformErrorKind;
use opencode_platform_api::fs::{
    LocationMutation, PathError, PathErrorReason, ResolveInput, ResolveKind, Target,
};
use opencode_platform_native::fs::NativeLocationMutation;
use std::path::{Path, PathBuf};

async fn location(dir: &Path) -> NativeLocationMutation {
    NativeLocationMutation::new(dir).await.unwrap()
}

async fn resolve(
    location: &NativeLocationMutation,
    path: impl AsRef<Path>,
    kind: Option<ResolveKind>,
) -> Result<Target, PathError> {
    location
        .resolve(ResolveInput {
            path: path.as_ref().to_string_lossy().into_owned(),
            kind,
        })
        .await
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

fn reason(error: PathError) -> PathErrorReason {
    match error {
        PathError::Invalid { reason, .. } => reason,
        other => panic!("expected PathError::Invalid, got {other:?}"),
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_links_preserve_location_and_external_approval_boundaries() {
    use opencode_platform_api::fs::FileMutation;
    use opencode_platform_native::fs::NativeFileMutation;

    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let inside = root.join("location");
    let outside = root.join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let loc = location(&inside).await;
    for (name, referent) in [
        ("relative", PathBuf::from("../outside/new/file.txt")),
        ("absolute", outside.join("new/file.txt")),
        ("chain", PathBuf::from("relative")),
        ("directory", PathBuf::from("../outside/missing")),
    ] {
        std::os::unix::fs::symlink(referent, inside.join(name)).unwrap();
        let path = if name == "directory" {
            "directory/file.txt"
        } else {
            name
        };
        assert_eq!(
            reason(resolve(&loc, path, None).await.unwrap_err()),
            PathErrorReason::LocationEscape,
            "{path}"
        );
    }
    assert!(!outside.join("new").exists());
    std::os::unix::fs::symlink("new/internal.txt", inside.join("internal")).unwrap();
    let internal = resolve(&loc, "internal", None).await.unwrap();
    assert_eq!(internal.canonical, inside.join("new/internal.txt"));
    assert!(internal.external_directory.is_none());
    NativeFileMutation::new()
        .write(&internal, b"internal")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(inside.join("new/internal.txt")).unwrap(),
        b"internal"
    );

    std::os::unix::fs::symlink("new/approved.txt", outside.join("link")).unwrap();
    let external = resolve(&loc, outside.join("link"), None).await.unwrap();
    assert_eq!(external.canonical, outside.join("new/approved.txt"));
    let approval = external.external_directory.as_ref().unwrap();
    assert_eq!(approval.directory, outside);
    assert_eq!(approval.permission().0, "external_directory");
    // A caller with explicit external approval can still perform the prospective write.
    NativeFileMutation::new()
        .write(&external, b"approved")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(outside.join("new/approved.txt")).unwrap(),
        b"approved"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn prospective_links_keep_parent_traversal_and_cycles_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let inside = root.join("location");
    let outside = root.join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(outside.join("sub")).unwrap();
    std::os::unix::fs::symlink(outside.join("sub"), inside.join("bridge")).unwrap();
    std::os::unix::fs::symlink("bridge/../missing.txt", inside.join("parent")).unwrap();
    let loc = location(&inside).await;
    assert_eq!(
        reason(resolve(&loc, "parent", None).await.unwrap_err()),
        PathErrorReason::LocationEscape
    );

    std::os::unix::fs::symlink("missing/../cycle", inside.join("cycle")).unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        resolve(&loc, "cycle/new.txt", None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error, PathError::Platform(_)));
    assert!(!outside.join("missing.txt").exists());
}

#[tokio::test]
async fn resolves_an_active_relative_existing_file_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "hello").unwrap();
    let target = resolve(&location(dir.path()).await, "hello.txt", None)
        .await
        .unwrap();
    assert_eq!(target.canonical, real(&dir.path().join("hello.txt")));
    assert_eq!(target.resource, "hello.txt");
    assert!(target.external_directory.is_none());
}

#[tokio::test]
async fn resolves_an_active_relative_prospective_file_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let target = resolve(&location(dir.path()).await, "src/new.txt", None)
        .await
        .unwrap();
    assert_eq!(
        target.canonical,
        real(dir.path()).join("src").join("new.txt")
    );
    assert_eq!(target.resource, "src/new.txt");
}

#[tokio::test]
async fn walks_several_missing_ancestors() {
    let dir = tempfile::tempdir().unwrap();
    let target = resolve(&location(dir.path()).await, "a/b/c/d.txt", None)
        .await
        .unwrap();
    assert_eq!(target.canonical, real(dir.path()).join("a/b/c/d.txt"));
    assert_eq!(target.resource, "a/b/c/d.txt");
}

#[tokio::test]
async fn location_root_resource_is_dot() {
    let dir = tempfile::tempdir().unwrap();
    let target = resolve(
        &location(dir.path()).await,
        ".",
        Some(ResolveKind::Directory),
    )
    .await
    .unwrap();
    assert_eq!(target.resource, ".");
    assert_eq!(target.canonical, real(dir.path()));
}

#[tokio::test]
async fn rejects_a_relative_lexical_escape() {
    let dir = tempfile::tempdir().unwrap();
    let error = resolve(&location(dir.path()).await, "../outside.txt", None)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        PathError::Invalid {
            path: "../outside.txt".into(),
            reason: PathErrorReason::RelativeEscape
        }
    );
    // Lexically internal `..` stays allowed.
    let ok = resolve(&location(dir.path()).await, "a/../b.txt", None)
        .await
        .unwrap();
    assert_eq!(ok.resource, "b.txt");
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_a_prospective_target_below_an_escaping_symlink_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    let error = resolve(&location(dir.path()).await, "escape/new.txt", None)
        .await
        .unwrap_err();
    assert_eq!(reason(error), PathErrorReason::LocationEscape);
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_an_existing_file_symlink_pointing_outside() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        dir.path().join("link.txt"),
    )
    .unwrap();
    let error = resolve(&location(dir.path()).await, "link.txt", None)
        .await
        .unwrap_err();
    assert_eq!(reason(error), PathErrorReason::LocationEscape);
}

#[cfg(unix)]
#[tokio::test]
async fn follows_an_in_location_symlink_using_ordinary_filesystem_semantics() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("actual")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("actual"), dir.path().join("linked")).unwrap();
    let target = resolve(&location(dir.path()).await, "linked/new.txt", None)
        .await
        .unwrap();
    assert_eq!(
        target.canonical,
        real(dir.path()).join("actual").join("new.txt")
    );
    assert_eq!(target.resource, "actual/new.txt");
}

/// Baseline parity: `realpath("file.txt/child")` fails with ENOTDIR (effect `BadResource`), which
/// the baseline `notFound` helper does not catch, so it surfaces a platform error rather than
/// `non_directory_ancestor` (location-mutation.ts:85-87, observed with Bun on macOS). The
/// `non_directory_ancestor` branch is only reachable when realpath reports ENOENT for a path whose
/// nearest existing ancestor is not a directory.
#[tokio::test]
async fn rejects_a_target_below_a_regular_file_with_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "x").unwrap();
    let loc = location(dir.path()).await;
    for path in ["file.txt/child.txt", "file.txt/deeper/child.txt"] {
        match resolve(&loc, path, None).await.unwrap_err() {
            PathError::Platform(error) => {
                assert_eq!(error.kind, PlatformErrorKind::NotADirectory, "{path}")
            }
            PathError::Invalid { reason, .. } => {
                assert_eq!(reason, PathErrorReason::NonDirectoryAncestor, "{path}")
            }
        }
    }
}

#[tokio::test]
async fn accepts_an_explicit_absolute_in_location_target_without_external_approval() {
    let dir = tempfile::tempdir().unwrap();
    let target = resolve(
        &location(dir.path()).await,
        dir.path().join("new.txt"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(target.canonical, real(dir.path()).join("new.txt"));
    assert_eq!(target.resource, "new.txt");
    assert!(target.external_directory.is_none());
}

#[tokio::test]
async fn requires_external_directory_authorization_for_an_explicit_external_absolute_target() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = resolve(
        &location(dir.path()).await,
        outside.path().join("new.txt"),
        None,
    )
    .await
    .unwrap();
    let root = real(outside.path());
    assert_eq!(target.canonical, root.join("new.txt"));
    assert_eq!(target.resource, slash(&root.join("new.txt")));
    let external = target.external_directory.unwrap();
    assert_eq!(external.directory, root);
    assert_eq!(external.resource, slash(&root.join("*")));
    assert_eq!(external.save, external.resource);
    let (action, resources, save) = external.permission();
    assert_eq!(action, "external_directory");
    assert_eq!(resources, vec![external.resource.clone()]);
    assert_eq!(save, vec![external.save.clone()]);
}

#[tokio::test]
async fn resolves_an_existing_external_file_target() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("existing.txt"), "existing").unwrap();
    let target = resolve(
        &location(dir.path()).await,
        outside.path().join("existing.txt"),
        None,
    )
    .await
    .unwrap();
    let root = real(outside.path());
    assert_eq!(target.canonical, root.join("existing.txt"));
    assert_eq!(target.external_directory.unwrap().directory, root);
}

#[tokio::test]
async fn anchors_prospective_external_descendants_at_their_stable_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = resolve(
        &location(dir.path()).await,
        outside.path().join("new/nested/file.txt"),
        None,
    )
    .await
    .unwrap();
    let root = real(outside.path());
    let external = target.external_directory.unwrap();
    assert_eq!(external.directory, root);
    assert_eq!(external.resource, slash(&root.join("*")));
}

#[tokio::test]
async fn directory_kind_uses_the_existing_directory_itself_as_external_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("sub")).unwrap();
    let loc = location(dir.path()).await;
    let root = real(outside.path());
    let as_dir = resolve(
        &loc,
        outside.path().join("sub"),
        Some(ResolveKind::Directory),
    )
    .await
    .unwrap();
    assert_eq!(
        as_dir.external_directory.unwrap().directory,
        root.join("sub")
    );
    let as_file = resolve(&loc, outside.path().join("sub"), Some(ResolveKind::File))
        .await
        .unwrap();
    // `kind: file` on an existing directory still anchors at the directory (info.type Directory).
    assert_eq!(
        as_file.external_directory.unwrap().directory,
        root.join("sub")
    );
}

/// Records observed behavior on case-insensitive volumes (default APFS): a differently-cased
/// relative path resolves to the on-disk canonical name, so both spellings share one lock key.
/// On case-sensitive volumes the other spelling is a distinct prospective target.
#[tokio::test]
async fn case_variant_paths_follow_the_volume_semantics() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Readme.md"), "x").unwrap();
    let insensitive = dir.path().join("README.MD").exists();
    let loc = location(dir.path()).await;
    let exact = resolve(&loc, "Readme.md", None).await.unwrap();
    let variant = resolve(&loc, "README.MD", None).await.unwrap();
    if insensitive {
        assert_eq!(variant.canonical, exact.canonical);
        assert_eq!(variant.resource, "Readme.md");
        return;
    }
    assert_ne!(variant.canonical, exact.canonical);
    assert_eq!(variant.resource, "README.MD");
}
