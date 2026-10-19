//! Purpose: real filesystem checks for legacy instance directory selection.
//! Scope: temporary local directories; no service fixtures.
//! Uses:
//! rust/crates/
//! `-- server/src/middleware/legacy_routing.rs
//! External:
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: Session HTTP and directory SSE share canonical instance identities.
//! Contract tests: this file.

use opencode_server::middleware::legacy_routing::{RouteContext, instance};

#[tokio::test]
async fn instance_resolves_dot_segments_missing_paths_and_encoded_directories() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().canonicalize().unwrap();
    std::fs::create_dir(real.join("space dir")).unwrap();
    let context = instance(RouteContext {
        directory: format!("{}/space%20dir", root.path().display()),
        workspace_id: Some("wrk_local".into()),
    })
    .await
    .unwrap();
    assert_eq!(context.directory, real.join("space dir").to_string_lossy());
    assert_eq!(context.workspace_id.as_deref(), Some("wrk_local"));
    let missing = instance(RouteContext {
        directory: format!("{}/space dir/../missing/./child", real.display()),
        workspace_id: None,
    })
    .await
    .unwrap();
    assert_eq!(
        missing.directory,
        real.join("missing/child").to_string_lossy()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_instance_uses_target_identity() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("target")).unwrap();
    std::os::unix::fs::symlink(root.path().join("target"), root.path().join("link")).unwrap();
    let context = instance(RouteContext {
        directory: root.path().join("link").to_string_lossy().into_owned(),
        workspace_id: None,
    })
    .await
    .unwrap();
    assert_eq!(
        context.directory,
        root.path()
            .join("target")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
    );
}
